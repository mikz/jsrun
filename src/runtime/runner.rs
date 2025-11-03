//! Runtime thread backed by `deno_core::JsRuntime`.
//!
//! This module hosts the JavaScript engine on a dedicated OS thread with a
//! single-threaded Tokio runtime. Commands from Python are forwarded through
//! [`RuntimeCommand`] and executed sequentially on that thread.

use crate::runtime::config::RuntimeConfig;
use crate::runtime::js_value::{JSValue, LimitTracker, MAX_JS_BYTES, MAX_JS_DEPTH};
use crate::runtime::loader::PythonModuleLoader;
use crate::runtime::ops::{python_extension, PythonOpMode, PythonOpRegistry};
use deno_core::error::CoreError;
use deno_core::{v8, JsRuntime, PollEventLoopOptions, RuntimeOptions};
use indexmap::IndexMap;
use pyo3::prelude::Py;
use pyo3::PyAny;
use pyo3_async_runtimes::TaskLocals;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::mpsc::Receiver as StdReceiver;
use std::sync::mpsc::Sender as StdSender;
use std::sync::mpsc::Sender;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::sync::oneshot;

type InitSignalChannel = (
    StdSender<Result<(), String>>,
    StdReceiver<Result<(), String>>,
);

/// Stored function with optional receiver for 'this' binding
struct StoredFunction {
    function: v8::Global<v8::Function>,
    receiver: Option<v8::Global<v8::Value>>,
}

/// Commands sent to the runtime thread.
pub enum RuntimeCommand {
    Eval {
        code: String,
        responder: Sender<Result<JSValue, String>>,
    },
    EvalAsync {
        code: String,
        timeout_ms: Option<u64>,
        task_locals: Option<TaskLocals>,
        responder: oneshot::Sender<Result<JSValue, String>>,
    },
    EvalModule {
        specifier: String,
        responder: Sender<Result<JSValue, String>>,
    },
    EvalModuleAsync {
        specifier: String,
        timeout_ms: Option<u64>,
        task_locals: Option<TaskLocals>,
        responder: oneshot::Sender<Result<JSValue, String>>,
    },
    RegisterPythonOp {
        name: String,
        mode: PythonOpMode,
        handler: Py<PyAny>,
        responder: Sender<Result<u32, String>>,
    },
    SetModuleResolver {
        handler: Py<PyAny>,
        responder: Sender<Result<(), String>>,
    },
    SetModuleLoader {
        handler: Py<PyAny>,
        responder: Sender<Result<(), String>>,
    },
    AddStaticModule {
        name: String,
        source: String,
        responder: Sender<Result<(), String>>,
    },
    CallFunctionAsync {
        fn_id: u32,
        args: Vec<JSValue>,
        timeout_ms: Option<u64>,
        task_locals: Option<TaskLocals>,
        responder: oneshot::Sender<Result<JSValue, String>>,
    },
    ReleaseFunction {
        fn_id: u32,
        responder: oneshot::Sender<Result<(), String>>,
    },
    Shutdown {
        responder: Sender<()>,
    },
}

pub fn spawn_runtime_thread(
    config: RuntimeConfig,
) -> Result<mpsc::UnboundedSender<RuntimeCommand>, String> {
    let (cmd_tx, cmd_rx) = mpsc::unbounded_channel::<RuntimeCommand>();
    let (init_tx, init_rx): InitSignalChannel = std::sync::mpsc::channel();

    std::thread::Builder::new()
        .name("jsrun-deno-runtime".to_string())
        .spawn(move || {
            let tokio_rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("failed to build tokio runtime");

            let mut core = match RuntimeCore::new(config) {
                Ok(core) => {
                    let _ = init_tx.send(Ok(()));
                    core
                }
                Err(err) => {
                    let _ = init_tx.send(Err(err));
                    return;
                }
            };

            tokio_rt.block_on(async move {
                core.run(cmd_rx).await;
            });
        })
        .map_err(|e| format!("Failed to spawn runtime thread: {}", e))?;

    match init_rx.recv() {
        Ok(Ok(())) => Ok(cmd_tx),
        Ok(Err(err)) => Err(err),
        Err(_) => Err("Runtime thread initialization failed".to_string()),
    }
}

struct RuntimeCore {
    js_runtime: JsRuntime,
    registry: PythonOpRegistry,
    module_loader: Rc<PythonModuleLoader>,
    task_locals: Option<TaskLocals>,
    execution_timeout: Option<Duration>,
    fn_registry: Rc<RefCell<HashMap<u32, StoredFunction>>>,
    next_fn_id: Rc<RefCell<u32>>,
}

impl RuntimeCore {
    fn new(config: RuntimeConfig) -> Result<Self, String> {
        let registry = PythonOpRegistry::new();
        let extension = python_extension(registry.clone());
        let module_loader = Rc::new(PythonModuleLoader::new());

        let RuntimeConfig {
            max_heap_size,
            initial_heap_size,
            execution_timeout,
            bootstrap_script,
        } = config;

        if initial_heap_size.is_some() && max_heap_size.is_none() {
            return Err("initial_heap_size requires max_heap_size to be set as well".to_string());
        }

        if let (Some(initial), Some(max)) = (initial_heap_size, max_heap_size) {
            if initial > max {
                return Err(format!(
                    "initial_heap_size ({}) cannot exceed max_heap_size ({})",
                    initial, max
                ));
            }
        }

        let create_params = match (max_heap_size, initial_heap_size) {
            (Some(max), initial) => {
                let initial_bytes = initial.unwrap_or(0);
                Some(v8::CreateParams::default().heap_limits(initial_bytes, max))
            }
            (None, _) => None,
        };

        let mut js_runtime = JsRuntime::new(RuntimeOptions {
            extensions: vec![extension],
            create_params,
            module_loader: Some(module_loader.clone()),
            ..Default::default()
        });

        if let Some(script) = bootstrap_script {
            js_runtime
                .execute_script("<bootstrap>", script)
                .map_err(|err| err.to_string())?;
        }

        Ok(Self {
            js_runtime,
            registry,
            module_loader,
            task_locals: None,
            execution_timeout,
            fn_registry: Rc::new(RefCell::new(HashMap::new())),
            next_fn_id: Rc::new(RefCell::new(0)),
        })
    }

    async fn run(&mut self, mut rx: mpsc::UnboundedReceiver<RuntimeCommand>) {
        while let Some(cmd) = rx.recv().await {
            match cmd {
                RuntimeCommand::Eval { code, responder } => {
                    let result = self.eval_sync(&code);
                    let _ = responder.send(result);
                }
                RuntimeCommand::EvalAsync {
                    code,
                    timeout_ms,
                    task_locals,
                    responder,
                } => {
                    // Update task_locals if provided
                    if let Some(ref locals) = task_locals {
                        self.task_locals = Some(locals.clone());
                        self.module_loader.set_task_locals(locals.clone());
                        // Update the OpState with the new task_locals
                        self.js_runtime
                            .op_state()
                            .borrow_mut()
                            .put(crate::runtime::ops::GlobalTaskLocals(Some(locals.clone())));
                    }
                    let result = self.eval_async(code, timeout_ms).await;
                    let _ = responder.send(result);
                }
                RuntimeCommand::RegisterPythonOp {
                    name,
                    mode,
                    handler,
                    responder,
                } => {
                    let result = self.register_python_op(name, mode, handler);
                    let _ = responder.send(result);
                }
                RuntimeCommand::SetModuleResolver { handler, responder } => {
                    self.module_loader.set_resolver(handler);
                    if let Some(ref locals) = self.task_locals {
                        self.module_loader.set_task_locals(locals.clone());
                    }
                    let _ = responder.send(Ok(()));
                }
                RuntimeCommand::SetModuleLoader { handler, responder } => {
                    self.module_loader.set_loader(handler);
                    if let Some(ref locals) = self.task_locals {
                        self.module_loader.set_task_locals(locals.clone());
                    }
                    let _ = responder.send(Ok(()));
                }
                RuntimeCommand::AddStaticModule {
                    name,
                    source,
                    responder,
                } => {
                    self.module_loader.add_static_module(name, source);
                    let _ = responder.send(Ok(()));
                }
                RuntimeCommand::EvalModule {
                    specifier,
                    responder,
                } => {
                    let result = self.eval_module_sync(&specifier);
                    let _ = responder.send(result);
                }
                RuntimeCommand::EvalModuleAsync {
                    specifier,
                    timeout_ms,
                    task_locals,
                    responder,
                } => {
                    // Update task_locals if provided
                    if let Some(ref locals) = task_locals {
                        self.task_locals = Some(locals.clone());
                        self.module_loader.set_task_locals(locals.clone());
                        // Update the OpState with the new task_locals
                        self.js_runtime
                            .op_state()
                            .borrow_mut()
                            .put(crate::runtime::ops::GlobalTaskLocals(Some(locals.clone())));
                    }
                    let result = self.eval_module_async(specifier, timeout_ms).await;
                    let _ = responder.send(result);
                }
                RuntimeCommand::CallFunctionAsync {
                    fn_id,
                    args,
                    timeout_ms,
                    task_locals,
                    responder,
                } => {
                    if let Some(ref locals) = task_locals {
                        self.task_locals = Some(locals.clone());
                        self.module_loader.set_task_locals(locals.clone());
                        self.js_runtime
                            .op_state()
                            .borrow_mut()
                            .put(crate::runtime::ops::GlobalTaskLocals(Some(locals.clone())));
                    }
                    let result = self.call_function_async(fn_id, args, timeout_ms).await;
                    let _ = responder.send(result);
                }
                RuntimeCommand::ReleaseFunction { fn_id, responder } => {
                    let result = self.release_function(fn_id);
                    let _ = responder.send(result);
                }
                RuntimeCommand::Shutdown { responder } => {
                    let leaked_count = self.fn_registry.borrow().len();
                    if leaked_count > 0 {
                        eprintln!(
                            "[jsrun] Warning: {} function handles not released before shutdown",
                            leaked_count
                        );
                    }
                    self.fn_registry.borrow_mut().clear();
                    let _ = responder.send(());
                    break;
                }
            }
        }
    }

    fn register_python_op(
        &self,
        name: String,
        mode: PythonOpMode,
        handler: Py<PyAny>,
    ) -> Result<u32, String> {
        Ok(self.registry.register(name, mode, handler))
    }

    fn eval_sync(&mut self, code: &str) -> Result<JSValue, String> {
        let global_value = self
            .js_runtime
            .execute_script("<eval>", code.to_string())
            .map_err(|err| err.to_string())?;

        let scope = &mut self.js_runtime.handle_scope();
        let local = deno_core::v8::Local::new(scope, global_value);
        Self::value_to_js_value(&self.fn_registry, &self.next_fn_id, scope, local)
    }

    async fn eval_async(
        &mut self,
        code: String,
        timeout_ms: Option<u64>,
    ) -> Result<JSValue, String> {
        let timeout_ms = timeout_ms.or_else(|| {
            self.execution_timeout.map(|duration| {
                let millis = duration.as_millis();
                if millis > u128::from(u64::MAX) {
                    u64::MAX
                } else {
                    millis as u64
                }
            })
        });

        let global_value = self
            .js_runtime
            .execute_script("<eval_async>", code.clone())
            .map_err(|err| err.to_string())?;

        let resolve_future = self.js_runtime.resolve(global_value);
        let poll_options = PollEventLoopOptions::default();

        let resolved = if let Some(ms) = timeout_ms {
            tokio::time::timeout(
                Duration::from_millis(ms),
                self.js_runtime
                    .with_event_loop_promise(resolve_future, poll_options),
            )
            .await
            .map_err(|_| format!("Evaluation timed out after {}ms", ms))?
            .map_err(to_string)?
        } else {
            self.js_runtime
                .with_event_loop_promise(resolve_future, poll_options)
                .await
                .map_err(to_string)?
        };

        let scope = &mut self.js_runtime.handle_scope();
        let local = deno_core::v8::Local::new(scope, resolved);
        Self::value_to_js_value(&self.fn_registry, &self.next_fn_id, scope, local)
    }

    fn eval_module_sync(&mut self, specifier: &str) -> Result<JSValue, String> {
        // Try to parse as absolute URL first, if it fails, resolve it as a bare specifier
        let module_specifier = if specifier.contains(':') || specifier.starts_with('/') {
            // Already a URL or absolute path
            deno_core::ModuleSpecifier::parse(specifier)
                .map_err(|e| format!("Invalid module specifier '{}': {}", specifier, e))?
        } else {
            // Bare specifier - resolve relative to a synthetic base
            let base = deno_core::ModuleSpecifier::parse("jsrun://runtime/")
                .map_err(|e| format!("Failed to create base URL: {}", e))?;
            base.join(specifier)
                .map_err(|e| format!("Failed to resolve module specifier '{}': {}", specifier, e))?
        };

        // Load the module
        let module_id =
            futures::executor::block_on(self.js_runtime.load_main_es_module(&module_specifier))
                .map_err(|e| format!("Failed to load module '{}': {}", specifier, e))?;

        // Evaluate the module
        let receiver = self.js_runtime.mod_evaluate(module_id);

        // Poll the runtime until the module evaluation completes
        let poll_options = PollEventLoopOptions::default();
        futures::executor::block_on(self.js_runtime.run_event_loop(poll_options))
            .map_err(|e| e.to_string())?;

        // Wait for the evaluation result - receiver returns Result<(), CoreError>
        let eval_result = futures::executor::block_on(receiver);

        // Check if evaluation succeeded
        if let Err(err) = eval_result {
            return Err(format!("Module evaluation failed: {}", err));
        }

        // Get the module namespace - must call get_module_namespace before handle_scope
        let module_namespace = self
            .js_runtime
            .get_module_namespace(module_id)
            .map_err(|e| format!("Failed to get module namespace: {}", e))?;
        let scope = &mut self.js_runtime.handle_scope();
        let local = deno_core::v8::Local::new(scope, module_namespace);
        Self::value_to_js_value(&self.fn_registry, &self.next_fn_id, scope, local.into())
    }

    async fn eval_module_async(
        &mut self,
        specifier: String,
        timeout_ms: Option<u64>,
    ) -> Result<JSValue, String> {
        let timeout_ms = timeout_ms.or_else(|| {
            self.execution_timeout.map(|duration| {
                let millis = duration.as_millis();
                if millis > u128::from(u64::MAX) {
                    u64::MAX
                } else {
                    millis as u64
                }
            })
        });

        // Try to parse as absolute URL first, if it fails, resolve it as a bare specifier
        let module_specifier = if specifier.contains(':') || specifier.starts_with('/') {
            // Already a URL or absolute path
            deno_core::ModuleSpecifier::parse(&specifier)
                .map_err(|e| format!("Invalid module specifier '{}': {}", specifier, e))?
        } else {
            // Bare specifier - resolve relative to a synthetic base
            let base = deno_core::ModuleSpecifier::parse("jsrun://runtime/")
                .map_err(|e| format!("Failed to create base URL: {}", e))?;
            base.join(&specifier)
                .map_err(|e| format!("Failed to resolve module specifier '{}': {}", specifier, e))?
        };

        // Load the module
        let module_id = self
            .js_runtime
            .load_main_es_module(&module_specifier)
            .await
            .map_err(|e| format!("Failed to load module '{}': {}", specifier, e))?;

        // Evaluate the module
        let receiver = self.js_runtime.mod_evaluate(module_id);

        // Poll the runtime until the module evaluation completes
        let poll_options = PollEventLoopOptions::default();

        if let Some(ms) = timeout_ms {
            tokio::time::timeout(
                Duration::from_millis(ms),
                self.js_runtime.run_event_loop(poll_options),
            )
            .await
            .map_err(|_| format!("Module evaluation timed out after {}ms", ms))?
            .map_err(|e| e.to_string())?
        } else {
            self.js_runtime
                .run_event_loop(poll_options)
                .await
                .map_err(|e| e.to_string())?
        };

        // Wait for the evaluation result - receiver returns Result<(), CoreError>
        let eval_result = receiver.await;

        // Check if evaluation succeeded
        if let Err(err) = eval_result {
            return Err(format!("Module evaluation failed: {}", err));
        }

        // Get the module namespace - must call get_module_namespace before handle_scope
        let module_namespace = self
            .js_runtime
            .get_module_namespace(module_id)
            .map_err(|e| format!("Failed to get module namespace: {}", e))?;
        let scope = &mut self.js_runtime.handle_scope();
        let local = deno_core::v8::Local::new(scope, module_namespace);
        Self::value_to_js_value(&self.fn_registry, &self.next_fn_id, scope, local.into())
    }

    /// Execute a stored JavaScript function by id with the provided arguments.
    async fn call_function_async(
        &mut self,
        fn_id: u32,
        args: Vec<JSValue>,
        timeout_ms: Option<u64>,
    ) -> Result<JSValue, String> {
        // Determine timeout
        let timeout_ms = timeout_ms.or_else(|| {
            self.execution_timeout.map(|duration| {
                let millis = duration.as_millis();
                if millis > u128::from(u64::MAX) {
                    u64::MAX
                } else {
                    millis as u64
                }
            })
        });

        // Look up function in registry and call it
        let result_global = {
            let registry = self.fn_registry.borrow();
            let stored = registry
                .get(&fn_id)
                .ok_or_else(|| format!("Function ID {} not found", fn_id))?;

            // Create scope and get function + receiver
            let scope = &mut self.js_runtime.handle_scope();
            let func = deno_core::v8::Local::new(scope, &stored.function);
            let receiver = stored
                .receiver
                .as_ref()
                .map(|r| deno_core::v8::Local::new(scope, r))
                .unwrap_or_else(|| scope.get_current_context().global(scope).into());

            // Convert arguments from JSValue to v8::Value
            let mut v8_args = Vec::with_capacity(args.len());
            for arg in args {
                let v8_val = match arg {
                    JSValue::Function { id } => {
                        // Look up the function in the registry and convert to v8::Function
                        let stored = registry
                            .get(&id)
                            .ok_or_else(|| format!("Function ID {} not found in args", id))?;
                        deno_core::v8::Local::new(scope, &stored.function).into()
                    }
                    _ => {
                        // Use serde_v8 for other types
                        deno_core::serde_v8::to_v8(scope, arg)
                            .map_err(|e| format!("Failed to convert argument: {}", e))?
                    }
                };
                v8_args.push(v8_val);
            }
            drop(registry); // Release borrow before function call

            // Call the function
            let result = func
                .call(scope, receiver, &v8_args)
                .ok_or_else(|| "Function call failed".to_string())?;

            // Convert to Global for async resolution
            deno_core::v8::Global::new(scope, result)
            // scope is dropped here
        };

        // Resolve promises (reuse eval_async pattern)
        let resolve_future = self.js_runtime.resolve(result_global);
        let poll_options = PollEventLoopOptions::default();

        let resolved = if let Some(ms) = timeout_ms {
            tokio::time::timeout(
                Duration::from_millis(ms),
                self.js_runtime
                    .with_event_loop_promise(resolve_future, poll_options),
            )
            .await
            .map_err(|_| format!("Function call timed out after {}ms", ms))?
            .map_err(to_string)?
        } else {
            self.js_runtime
                .with_event_loop_promise(resolve_future, poll_options)
                .await
                .map_err(to_string)?
        };

        // Convert back to JSValue
        let scope = &mut self.js_runtime.handle_scope();
        let local = deno_core::v8::Local::new(scope, resolved);
        Self::value_to_js_value(&self.fn_registry, &self.next_fn_id, scope, local)
    }

    /// Remove a function from the registry, freeing its V8 global handle.
    fn release_function(&mut self, fn_id: u32) -> Result<(), String> {
        let mut registry = self.fn_registry.borrow_mut();
        registry
            .remove(&fn_id)
            .ok_or_else(|| format!("Function ID {} not found", fn_id))?;
        Ok(())
    }

    /// Convert a V8 value to JSValue with circular reference detection and limits enforced.
    fn value_to_js_value<'s>(
        fn_registry: &Rc<RefCell<HashMap<u32, StoredFunction>>>,
        next_fn_id: &Rc<RefCell<u32>>,
        scope: &mut deno_core::v8::HandleScope<'s>,
        value: deno_core::v8::Local<'s, deno_core::v8::Value>,
    ) -> Result<JSValue, String> {
        let mut seen = HashSet::new();
        let mut tracker = LimitTracker::new(MAX_JS_DEPTH, MAX_JS_BYTES);
        Self::value_to_js_value_internal(
            fn_registry,
            next_fn_id,
            scope,
            value,
            &mut seen,
            &mut tracker,
            None,
        )
    }

    /// Internal recursive converter with cycle detection and optional receiver capture.
    fn value_to_js_value_internal<'s>(
        fn_registry: &Rc<RefCell<HashMap<u32, StoredFunction>>>,
        next_fn_id: &Rc<RefCell<u32>>,
        scope: &mut deno_core::v8::HandleScope<'s>,
        value: deno_core::v8::Local<'s, deno_core::v8::Value>,
        seen: &mut HashSet<i32>,
        tracker: &mut LimitTracker,
        receiver: Option<deno_core::v8::Global<deno_core::v8::Value>>,
    ) -> Result<JSValue, String> {
        tracker.enter()?;

        let result = if value.is_null() || value.is_undefined() {
            tracker.add_bytes(4)?; // "null" or "undefined"
            Ok(JSValue::Null)
        } else if value.is_boolean() {
            tracker.add_bytes(5)?; // "false" (worst case)
            Ok(JSValue::Bool(value.boolean_value(scope)))
        } else if value.is_number() {
            // Handle special numeric values (NaN, ±Infinity)
            let num_obj = value
                .to_number(scope)
                .ok_or_else(|| "Failed to convert value to number".to_string())?;
            let num_val = num_obj.value();
            if num_val.is_nan() || num_val.is_infinite() {
                tracker.add_bytes(24)?;
                Ok(JSValue::Float(num_val))
            } else if num_val.fract() == 0.0 && num_val.is_finite() {
                let as_int = num_val as i64;
                if as_int as f64 == num_val {
                    tracker.add_bytes(20)?;
                    Ok(JSValue::Int(as_int))
                } else {
                    tracker.add_bytes(24)?;
                    Ok(JSValue::Float(num_val))
                }
            } else {
                tracker.add_bytes(24)?;
                Ok(JSValue::Float(num_val))
            }
        } else if value.is_string() {
            let string = value
                .to_string(scope)
                .ok_or_else(|| "Failed to convert string".to_string())?;
            let rust_str = string.to_rust_string_lossy(scope);
            tracker.add_bytes(rust_str.len())?;
            Ok(JSValue::String(rust_str))
        } else if value.is_big_int() {
            // Try to convert BigInt to i64, otherwise error
            let bigint = deno_core::v8::Local::<deno_core::v8::BigInt>::try_from(value)
                .map_err(|_| "Failed to cast to BigInt".to_string())?;
            let (val, lossless) = bigint.i64_value();
            if lossless {
                tracker.add_bytes(20)?;
                Ok(JSValue::Int(val))
            } else {
                Err("BigInt value too large to represent as i64".to_string())
            }
        } else if value.is_function() {
            // Register function and return proxy ID
            let func = deno_core::v8::Local::<deno_core::v8::Function>::try_from(value)
                .map_err(|_| "Failed to cast to function".to_string())?;

            // Create a Global handle to keep the function alive
            let fn_handle = deno_core::v8::Global::new(scope, func);

            // Register in the function registry
            let mut registry = fn_registry.borrow_mut();
            let mut next_id_val = next_fn_id.borrow_mut();

            let fn_id = *next_id_val;
            *next_id_val += 1;

            registry.insert(
                fn_id,
                StoredFunction {
                    function: fn_handle,
                    receiver, // Capture receiver for 'this' binding
                },
            );

            tracker.add_bytes(8)?; // ID size
            Ok(JSValue::Function { id: fn_id })
        } else if value.is_symbol() {
            Err("Cannot serialize V8 symbol".to_string())
        } else if value.is_array() {
            // Check for circular reference using identity hash
            let obj = deno_core::v8::Local::<deno_core::v8::Object>::try_from(value)
                .map_err(|_| "Failed to cast array to object".to_string())?;
            let hash = obj.get_identity_hash().get();

            if !seen.insert(hash) {
                return Err("Cannot serialize circular reference".to_string());
            }

            let array = deno_core::v8::Local::<deno_core::v8::Array>::try_from(value)
                .map_err(|_| "Failed to cast to array".to_string())?;
            let len = array.length() as usize;

            let mut items = Vec::with_capacity(len);
            for i in 0..len {
                let idx = i as u32;
                let item = array
                    .get_index(scope, idx)
                    .ok_or_else(|| format!("Failed to get array index {}", i))?;
                items.push(Self::value_to_js_value_internal(
                    fn_registry,
                    next_fn_id,
                    scope,
                    item,
                    seen,
                    tracker,
                    None,
                )?);
            }

            seen.remove(&hash);
            Ok(JSValue::Array(items))
        } else if value.is_object() {
            // Check for circular reference using identity hash
            let obj = deno_core::v8::Local::<deno_core::v8::Object>::try_from(value)
                .map_err(|_| "Failed to cast to object".to_string())?;
            let hash = obj.get_identity_hash().get();

            if !seen.insert(hash) {
                return Err("Cannot serialize circular reference".to_string());
            }

            // Get property names
            let prop_names = obj
                .get_own_property_names(scope, deno_core::v8::GetPropertyNamesArgs::default())
                .ok_or_else(|| "Failed to get property names".to_string())?;

            let mut map = IndexMap::new();
            for i in 0..prop_names.length() {
                let key = prop_names
                    .get_index(scope, i)
                    .ok_or_else(|| "Failed to get property name".to_string())?;
                let key_str = key
                    .to_string(scope)
                    .ok_or_else(|| "Failed to convert key to string".to_string())?
                    .to_rust_string_lossy(scope);

                let val = obj
                    .get(scope, key)
                    .ok_or_else(|| format!("Failed to get property '{}'", key_str))?;

                // If the value is a function, capture the object as the receiver for 'this' binding
                let receiver_for_val = if val.is_function() {
                    let obj_as_value: deno_core::v8::Local<deno_core::v8::Value> = obj.into();
                    Some(deno_core::v8::Global::new(scope, obj_as_value))
                } else {
                    None
                };

                tracker.add_bytes(key_str.len())?;
                map.insert(
                    key_str,
                    Self::value_to_js_value_internal(
                        fn_registry,
                        next_fn_id,
                        scope,
                        val,
                        seen,
                        tracker,
                        receiver_for_val,
                    )?,
                );
            }

            seen.remove(&hash);
            Ok(JSValue::Object(map))
        } else {
            // Fallback: convert to string
            let string = value
                .to_string(scope)
                .ok_or_else(|| "Failed to convert value to string".to_string())?;
            let rust_str = string.to_rust_string_lossy(scope);
            tracker.add_bytes(rust_str.len())?;
            Ok(JSValue::String(rust_str))
        };

        tracker.exit();
        result
    }
}

fn to_string(error: CoreError) -> String {
    error.to_string()
}

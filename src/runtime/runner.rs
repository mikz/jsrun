//! Runtime thread bootstrap and execution loop.
//!
//! This module manages the runtime thread that hosts the V8 isolate and Tokio event loop.
//! Each runtime runs on a dedicated OS thread with a single-threaded Tokio runtime.

use super::config::RuntimeConfig;
use super::op_driver::{FuturesUnorderedDriver, OpDriver};
use std::convert::TryInto;
use std::ffi::c_void;
use std::sync::mpsc;
use std::task::{Context, Poll};
use std::thread;
use tokio::sync::mpsc as async_mpsc;

/// Commands sent from Python to the runtime thread.
pub enum HostCommand {
    /// Evaluate JavaScript code synchronously
    Eval {
        code: String,
        responder: mpsc::Sender<Result<String, String>>,
    },
    /// Evaluate JavaScript code asynchronously with optional timeout
    EvalAsync {
        code: String,
        timeout_ms: Option<u64>,
        responder: mpsc::Sender<Result<String, String>>,
    },
    /// Execute an ES module
    ExecuteModule {
        url: String,
        responder: mpsc::Sender<Result<String, String>>,
    },
    /// Drain pending tasks (microtasks, promises)
    DrainTasks {
        responder: mpsc::Sender<Result<(), String>>,
    },
    /// Call an async op
    OpCall {
        op_id: u32,
        promise_id: u32,
        args: Vec<serde_json::Value>,
        responder: mpsc::Sender<Result<serde_json::Value, String>>,
    },
    /// Register a new op
    RegisterOp {
        name: String,
        mode: super::ops::OpMode,
        permissions: Vec<super::ops::Permission>,
        handler: super::ops::OpHandler,
        responder: mpsc::Sender<Result<u32, String>>,
    },
    /// Create a new context
    CreateContext {
        responder: mpsc::Sender<Result<usize, String>>,
    },
    /// Drop a context
    DropContext { context_id: usize },
    /// Evaluate code in a specific context
    EvalInContext {
        context_id: usize,
        code: String,
        responder: mpsc::Sender<Result<String, String>>,
    },
    /// Evaluate code asynchronously in a specific context
    EvalInContextAsync {
        context_id: usize,
        code: String,
        timeout_ms: Option<u64>,
        responder: mpsc::Sender<Result<String, String>>,
    },
    /// Get the number of pending operations
    GetPendingOps { responder: mpsc::Sender<usize> },
    /// Shutdown the runtime
    Shutdown,
}

impl std::fmt::Debug for HostCommand {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Eval { code, .. } => f
                .debug_struct("Eval")
                .field(
                    "code",
                    &format!("{}...", &code.chars().take(50).collect::<String>()),
                )
                .finish(),
            Self::EvalAsync {
                code, timeout_ms, ..
            } => f
                .debug_struct("EvalAsync")
                .field(
                    "code",
                    &format!("{}...", &code.chars().take(50).collect::<String>()),
                )
                .field("timeout_ms", timeout_ms)
                .finish(),
            Self::ExecuteModule { url, .. } => {
                f.debug_struct("ExecuteModule").field("url", url).finish()
            }
            Self::DrainTasks { .. } => write!(f, "DrainTasks"),
            Self::OpCall {
                op_id,
                promise_id,
                args,
                ..
            } => f
                .debug_struct("OpCall")
                .field("op_id", op_id)
                .field("promise_id", promise_id)
                .field("args", args)
                .finish(),
            Self::RegisterOp {
                name,
                mode,
                permissions,
                ..
            } => f
                .debug_struct("RegisterOp")
                .field("name", name)
                .field("mode", mode)
                .field("permissions", permissions)
                .field("handler", &"<function>")
                .finish(),
            Self::CreateContext { .. } => write!(f, "CreateContext"),
            Self::DropContext { context_id } => f
                .debug_struct("DropContext")
                .field("context_id", context_id)
                .finish(),
            Self::EvalInContext {
                context_id, code, ..
            } => f
                .debug_struct("EvalInContext")
                .field("context_id", context_id)
                .field(
                    "code",
                    &format!("{}...", &code.chars().take(50).collect::<String>()),
                )
                .finish(),
            Self::EvalInContextAsync {
                context_id,
                code,
                timeout_ms,
                ..
            } => f
                .debug_struct("EvalInContextAsync")
                .field("context_id", context_id)
                .field(
                    "code",
                    &format!("{}...", &code.chars().take(50).collect::<String>()),
                )
                .field("timeout_ms", timeout_ms)
                .finish(),
            Self::GetPendingOps { .. } => write!(f, "GetPendingOps"),
            Self::Shutdown => write!(f, "Shutdown"),
        }
    }
}

/// Core JavaScript runtime state running on the runtime thread.
pub struct JsRuntimeCore {
    isolate: rusty_v8::OwnedIsolate,
    /// Primary context used for runtime.eval()
    primary_context: rusty_v8::Global<rusty_v8::Context>,
    /// Additional contexts created via runtime.create_context()
    contexts: std::collections::HashMap<usize, rusty_v8::Global<rusty_v8::Context>>,
    /// Next context ID to assign
    next_context_id: usize,
    /// Optional bootstrap script to run inside every context
    bootstrap_script: Option<String>,
    op_registry: std::rc::Rc<std::cell::RefCell<super::ops::OpRegistry>>,
    /// OpDriver for managing async operations (wrapped for interior mutability)
    op_driver: std::rc::Rc<std::cell::RefCell<FuturesUnorderedDriver>>,
}

/// Bootstrap JavaScript code for ops system.
const OPS_BOOTSTRAP: &str = include_str!("ops_bootstrap.js");

/// Embedder slot holding a pointer to the runtime's op registry.
const REGISTRY_EMBEDDER_SLOT: i32 = 0;

/// Embedder slot holding a pointer to the runtime's op driver.
const DRIVER_EMBEDDER_SLOT: i32 = 1;

fn get_op_registry(
    scope: &mut rusty_v8::HandleScope,
) -> Result<std::rc::Rc<std::cell::RefCell<super::ops::OpRegistry>>, String> {
    let context = scope.get_current_context();
    let ptr = context.get_aligned_pointer_from_embedder_data(REGISTRY_EMBEDDER_SLOT);
    if ptr.is_null() {
        return Err("Op registry not installed".to_string());
    }
    // SAFETY: The pointer was created from Rc::into_raw and is still valid
    // We reconstruct the Rc temporarily to clone it, then forget it to avoid decrementing the refcount
    let registry = unsafe {
        let rc = std::rc::Rc::from_raw(ptr as *const std::cell::RefCell<super::ops::OpRegistry>);
        let cloned = rc.clone();
        std::mem::forget(rc); // Don't drop the original Rc
        cloned
    };
    Ok(registry)
}

fn get_op_driver(
    scope: &mut rusty_v8::HandleScope,
) -> Result<std::rc::Rc<std::cell::RefCell<super::op_driver::FuturesUnorderedDriver>>, String> {
    let context = scope.get_current_context();
    let ptr = context.get_aligned_pointer_from_embedder_data(DRIVER_EMBEDDER_SLOT);
    if ptr.is_null() {
        return Err("Op driver not installed".to_string());
    }
    // SAFETY: The pointer was created from Rc::into_raw and is still valid
    let driver = unsafe {
        let rc = std::rc::Rc::from_raw(
            ptr as *const std::cell::RefCell<super::op_driver::FuturesUnorderedDriver>,
        );
        let cloned = rc.clone();
        std::mem::forget(rc); // Don't drop the original Rc
        cloned
    };
    Ok(driver)
}

fn json_to_v8<'s>(
    scope: &mut rusty_v8::HandleScope<'s>,
    value: &serde_json::Value,
) -> Result<rusty_v8::Local<'s, rusty_v8::Value>, String> {
    match value {
        serde_json::Value::Null => Ok(rusty_v8::null(scope).into()),
        serde_json::Value::Bool(b) => Ok(rusty_v8::Boolean::new(scope, *b).into()),
        serde_json::Value::Number(num) => {
            if let Some(i) = num.as_i64() {
                Ok(rusty_v8::Number::new(scope, i as f64).into())
            } else if let Some(u) = num.as_u64() {
                Ok(rusty_v8::Number::new(scope, u as f64).into())
            } else if let Some(f) = num.as_f64() {
                Ok(rusty_v8::Number::new(scope, f).into())
            } else {
                Err("Unsupported numeric value".to_string())
            }
        }
        serde_json::Value::String(s) => rusty_v8::String::new(scope, s)
            .map(|val| val.into())
            .ok_or_else(|| "Failed to create V8 string".to_string()),
        serde_json::Value::Array(items) => {
            let array = rusty_v8::Array::new(scope, items.len() as i32);
            for (index, item) in items.iter().enumerate() {
                let value = json_to_v8(scope, item)?;
                if !array.set_index(scope, index as u32, value).unwrap_or(false) {
                    return Err(format!("Failed to set array index {}", index));
                }
            }
            Ok(array.into())
        }
        serde_json::Value::Object(entries) => {
            let object = rusty_v8::Object::new(scope);
            for (key, entry_value) in entries {
                let key_str = rusty_v8::String::new(scope, key)
                    .ok_or_else(|| format!("Failed to create object key '{}'", key))?;
                let value = json_to_v8(scope, entry_value)?;
                if !object.set(scope, key_str.into(), value).unwrap_or(false) {
                    return Err(format!("Failed to set object property '{}'", key));
                }
            }
            Ok(object.into())
        }
    }
}

fn resolve_op_in_scope<'s>(
    scope: &mut rusty_v8::HandleScope<'s>,
    promise_id: u32,
    op_result: &Result<serde_json::Value, String>,
) -> Result<(), String> {
    let context = scope.get_current_context();
    let global = context.global(scope);

    let resolver_key = rusty_v8::String::new(scope, "__resolveOp")
        .ok_or_else(|| "Failed to create __resolveOp string".to_string())?;
    let resolver_value = global
        .get(scope, resolver_key.into())
        .ok_or_else(|| "__resolveOp not found".to_string())?;
    let resolver: rusty_v8::Local<rusty_v8::Function> = resolver_value
        .try_into()
        .map_err(|_| "__resolveOp is not a function".to_string())?;

    let promise_id_value = rusty_v8::Integer::new_from_unsigned(scope, promise_id);
    let status_value = rusty_v8::Boolean::new(scope, op_result.is_ok());
    let result_value = match op_result {
        Ok(value) => json_to_v8(scope, value)?,
        Err(err) => {
            let message = rusty_v8::String::new(scope, err)
                .ok_or_else(|| "Failed to create error string".to_string())?;
            rusty_v8::Exception::error(scope, message)
        }
    };

    let args = [promise_id_value.into(), status_value.into(), result_value];

    resolver
        .call(scope, global.into(), &args)
        .ok_or_else(|| "Failed to invoke __resolveOp".to_string())
        .map(|_| ())
}

/// V8 callback for __host_op_async_impl__ function.
/// This is called from JavaScript when an async op is invoked.
fn host_op_async_impl_callback(
    scope: &mut rusty_v8::HandleScope,
    args: rusty_v8::FunctionCallbackArguments,
    _rv: rusty_v8::ReturnValue,
) {
    // Extract arguments: opId, promiseId, ...args
    if args.length() < 2 {
        let msg = rusty_v8::String::new(scope, "Expected at least 2 arguments (opId, promiseId)")
            .unwrap();
        let exception = rusty_v8::Exception::type_error(scope, msg);
        scope.throw_exception(exception);
        return;
    }

    let op_id = match args.get(0).uint32_value(scope) {
        Some(id) => id,
        None => {
            let msg = rusty_v8::String::new(scope, "opId must be a number").unwrap();
            let exception = rusty_v8::Exception::type_error(scope, msg);
            scope.throw_exception(exception);
            return;
        }
    };

    let promise_id = match args.get(1).uint32_value(scope) {
        Some(id) => id,
        None => {
            let msg = rusty_v8::String::new(scope, "promiseId must be a number").unwrap();
            let exception = rusty_v8::Exception::type_error(scope, msg);
            scope.throw_exception(exception);
            return;
        }
    };

    // Convert remaining arguments to JSON
    let mut json_args = Vec::new();
    for i in 2..args.length() {
        let arg = args.get(i);
        match v8_to_json(scope, arg) {
            Ok(json_value) => json_args.push(json_value),
            Err(err) => {
                let msg = rusty_v8::String::new(
                    scope,
                    &format!("Failed to convert argument {}: {}", i, err),
                )
                .unwrap();
                let exception = rusty_v8::Exception::error(scope, msg);
                scope.throw_exception(exception);
                return;
            }
        }
    }

    let registry_rc = match get_op_registry(scope) {
        Ok(registry) => registry,
        Err(err) => {
            let msg = rusty_v8::String::new(scope, &err).unwrap();
            let exception = rusty_v8::Exception::error(scope, msg);
            scope.throw_exception(exception);
            return;
        }
    };

    // Get the OpDriver for async operation submission
    let driver_rc = match get_op_driver(scope) {
        Ok(driver) => driver,
        Err(err) => {
            let msg = rusty_v8::String::new(scope, &err).unwrap();
            let exception = rusty_v8::Exception::error(scope, msg);
            scope.throw_exception(exception);
            return;
        }
    };

    // Get the op outcome (sync or async)
    let outcome = {
        let registry = registry_rc.borrow();
        registry.call_op_outcome(op_id, json_args)
    };

    match outcome {
        Ok(super::ops::OpCallOutcome::Sync(result)) => {
            // Synchronous result - resolve immediately
            if let Err(err) = resolve_op_in_scope(scope, promise_id, &result) {
                let msg = rusty_v8::String::new(scope, &err).unwrap();
                let exception = rusty_v8::Exception::error(scope, msg);
                scope.throw_exception(exception);
            }
        }
        Ok(super::ops::OpCallOutcome::Async { future, scheduling }) => {
            // Async operation - submit to driver for polling.
            // Poll once immediately for eager scheduling; if not ready, queue it.
            let driver_ref = driver_rc.borrow();

            if driver_ref.is_shutdown() {
                drop(driver_ref);
                let shutdown_err = Err("Runtime is shutting down".to_string());
                if let Err(resolve_err) = resolve_op_in_scope(scope, promise_id, &shutdown_err) {
                    let msg = rusty_v8::String::new(scope, &resolve_err).unwrap();
                    let exception = rusty_v8::Exception::error(scope, msg);
                    scope.throw_exception(exception);
                }
                return;
            }

            let eager_result = driver_ref.submit_op_fallible(op_id, promise_id, scheduling, future);
            drop(driver_ref);

            // If the future completed eagerly, resolve the promise immediately.
            if let Some(result) = eager_result {
                if let Err(resolve_err) = resolve_op_in_scope(scope, promise_id, &result) {
                    let msg = rusty_v8::String::new(scope, &resolve_err).unwrap();
                    let exception = rusty_v8::Exception::error(scope, msg);
                    scope.throw_exception(exception);
                }
            }
        }
        Err(err) => {
            // Permission error or op not found - reject the promise
            let result = Err(err.clone());
            if let Err(resolve_err) = resolve_op_in_scope(scope, promise_id, &result) {
                let msg = rusty_v8::String::new(scope, &resolve_err).unwrap();
                let exception = rusty_v8::Exception::error(scope, msg);
                scope.throw_exception(exception);
            }
        }
    }
}

/// V8 callback for __host_op_sync__ function.
fn host_op_sync_callback(
    scope: &mut rusty_v8::HandleScope,
    args: rusty_v8::FunctionCallbackArguments,
    mut rv: rusty_v8::ReturnValue,
) {
    if args.length() < 1 {
        let msg = rusty_v8::String::new(scope, "Expected at least 1 argument (opId)").unwrap();
        let exception = rusty_v8::Exception::type_error(scope, msg);
        scope.throw_exception(exception);
        return;
    }

    let op_id = match args.get(0).uint32_value(scope) {
        Some(id) => id,
        None => {
            let msg = rusty_v8::String::new(scope, "opId must be a number").unwrap();
            let exception = rusty_v8::Exception::type_error(scope, msg);
            scope.throw_exception(exception);
            return;
        }
    };

    let mut json_args = Vec::new();
    for i in 1..args.length() {
        let arg = args.get(i);
        match v8_to_json(scope, arg) {
            Ok(json_value) => json_args.push(json_value),
            Err(err) => {
                let msg = rusty_v8::String::new(
                    scope,
                    &format!("Failed to convert argument {}: {}", i, err),
                )
                .unwrap();
                let exception = rusty_v8::Exception::error(scope, msg);
                scope.throw_exception(exception);
                return;
            }
        }
    }

    let registry_rc = match get_op_registry(scope) {
        Ok(registry) => registry,
        Err(err) => {
            let msg = rusty_v8::String::new(scope, &err).unwrap();
            let exception = rusty_v8::Exception::error(scope, msg);
            scope.throw_exception(exception);
            return;
        }
    };
    {
        let registry = registry_rc.borrow();
        if let Some(metadata) = registry.get_by_id(op_id) {
            if metadata.mode == super::ops::OpMode::Async {
                let msg = rusty_v8::String::new(
                    scope,
                    "Op requires async invocation (__host_op_async__)",
                )
                .unwrap();
                let exception = rusty_v8::Exception::type_error(scope, msg);
                scope.throw_exception(exception);
                return;
            }
        } else {
            let msg = rusty_v8::String::new(scope, &format!("Op {} not found", op_id)).unwrap();
            let exception = rusty_v8::Exception::error(scope, msg);
            scope.throw_exception(exception);
            return;
        }
    }

    let call_result = {
        let registry = registry_rc.borrow();
        registry.call_op(op_id, json_args)
    };

    match call_result {
        Ok(value) => match json_to_v8(scope, &value) {
            Ok(v8_value) => rv.set(v8_value),
            Err(err) => {
                let msg = rusty_v8::String::new(scope, &err).unwrap();
                let exception = rusty_v8::Exception::error(scope, msg);
                scope.throw_exception(exception);
            }
        },
        Err(err) => {
            let msg = rusty_v8::String::new(scope, &err).unwrap();
            let exception = rusty_v8::Exception::error(scope, msg);
            scope.throw_exception(exception);
        }
    }
}

/// Convert V8 value to JSON.
fn v8_to_json(
    scope: &mut rusty_v8::HandleScope,
    value: rusty_v8::Local<rusty_v8::Value>,
) -> Result<serde_json::Value, String> {
    if value.is_null() || value.is_undefined() {
        Ok(serde_json::Value::Null)
    } else if value.is_boolean() {
        Ok(serde_json::Value::Bool(value.boolean_value(scope)))
    } else if value.is_number() {
        let num = value
            .number_value(scope)
            .ok_or("Failed to get number value")?;
        Ok(serde_json::Number::from_f64(num)
            .map(serde_json::Value::Number)
            .unwrap_or(serde_json::Value::Null))
    } else if value.is_string() {
        let s = value.to_rust_string_lossy(scope);
        Ok(serde_json::Value::String(s))
    } else if value.is_array() {
        let array: rusty_v8::Local<rusty_v8::Array> = value.cast();
        let len = array.length();
        let mut items = Vec::new();
        for i in 0..len {
            let item = array
                .get_index(scope, i)
                .ok_or("Failed to get array item")?;
            items.push(v8_to_json(scope, item)?);
        }
        Ok(serde_json::Value::Array(items))
    } else if value.is_object() {
        let obj: rusty_v8::Local<rusty_v8::Object> = value.cast();
        let prop_names = obj
            .get_own_property_names(scope, Default::default())
            .ok_or("Failed to get property names")?;
        let len = prop_names.length();
        let mut map = serde_json::Map::new();
        for i in 0..len {
            let key = prop_names.get_index(scope, i).ok_or("Failed to get key")?;
            let key_str = key.to_rust_string_lossy(scope);
            let val = obj.get(scope, key).ok_or("Failed to get property value")?;
            map.insert(key_str, v8_to_json(scope, val)?);
        }
        Ok(serde_json::Value::Object(map))
    } else {
        Err("Unsupported V8 value type".to_string())
    }
}

impl JsRuntimeCore {
    /// Create a new JavaScript runtime core with the given configuration.
    pub fn new(config: &RuntimeConfig) -> Result<Self, String> {
        // Ensure V8 platform is initialized
        super::initialize_platform_once();

        // Create isolate with custom parameters
        let mut params = rusty_v8::CreateParams::default();

        // Configure heap limits if specified
        if let Some(initial) = config.initial_heap_size {
            params = params.heap_limits(initial, config.max_heap_size.unwrap_or(initial * 2));
        } else if let Some(max) = config.max_heap_size {
            params = params.heap_limits(0, max);
        }

        let mut isolate = rusty_v8::Isolate::new(params);

        // Create a new context
        let context = {
            let scope = &mut rusty_v8::HandleScope::new(&mut isolate);
            let context = rusty_v8::Context::new(scope, Default::default());
            rusty_v8::Global::new(scope, context)
        };

        let mut core = Self {
            isolate,
            primary_context: context,
            contexts: std::collections::HashMap::new(),
            next_context_id: 0,
            bootstrap_script: config.bootstrap_script.clone(),
            op_registry: std::rc::Rc::new(std::cell::RefCell::new(super::ops::OpRegistry::new())),
            op_driver: std::rc::Rc::new(std::cell::RefCell::new(FuturesUnorderedDriver::default())),
        };

        core.initialize(config)?;

        Ok(core)
    }

    fn initialize(&mut self, config: &RuntimeConfig) -> Result<(), String> {
        // Seed granted permissions
        for permission in &config.permissions {
            self.op_registry
                .borrow_mut()
                .grant_permission(super::ops::Permission::from(permission));
        }

        let primary_context = self.primary_context.clone();
        self.bootstrap_ops_for_context(&primary_context)?;
        if let Err(err) = self.run_user_bootstrap(&primary_context) {
            self.clear_context_registry_pointer(&primary_context);
            return Err(err);
        }

        Ok(())
    }

    /// Evaluate JavaScript code and return the result (synchronous).
    pub fn eval(&mut self, code: &str) -> Result<String, String> {
        let scope = &mut rusty_v8::HandleScope::new(&mut self.isolate);
        let context = rusty_v8::Local::new(scope, &self.primary_context);
        let scope = &mut rusty_v8::ContextScope::new(scope, context);

        let code_str = rusty_v8::String::new(scope, code)
            .ok_or_else(|| "Failed to create code string".to_string())?;

        let script = rusty_v8::Script::compile(scope, code_str, None)
            .ok_or_else(|| "Failed to compile script".to_string())?;

        let result = script
            .run(scope)
            .ok_or_else(|| "Script execution failed".to_string())?;

        let result_str = result
            .to_string(scope)
            .ok_or_else(|| "Failed to convert result to string".to_string())?;

        Ok(result_str.to_rust_string_lossy(scope))
    }

    /// Evaluate JavaScript code asynchronously with optional timeout.
    ///
    /// This method evaluates code that may return a promise and polls it to completion.
    /// It performs microtask checkpoints to ensure promises are resolved.
    pub async fn eval_async(
        &mut self,
        code: &str,
        timeout_ms: Option<u64>,
    ) -> Result<String, String> {
        // Compile and execute the script
        let promise = {
            let scope = &mut rusty_v8::HandleScope::new(&mut self.isolate);
            let context = rusty_v8::Local::new(scope, &self.primary_context);
            let scope = &mut rusty_v8::ContextScope::new(scope, context);

            let code_str = rusty_v8::String::new(scope, code)
                .ok_or_else(|| "Failed to create code string".to_string())?;

            let script = rusty_v8::Script::compile(scope, code_str, None)
                .ok_or_else(|| "Failed to compile script".to_string())?;

            let result = script
                .run(scope)
                .ok_or_else(|| "Script execution failed".to_string())?;

            // If the result is a promise, store it globally for polling
            if result.is_promise() {
                let promise: rusty_v8::Local<rusty_v8::Promise> = result
                    .try_into()
                    .map_err(|_| "Failed to cast promise result".to_string())?;
                Some(rusty_v8::Global::new(scope, promise))
            } else {
                // Non-promise result: convert immediately
                let result_str = result
                    .to_string(scope)
                    .ok_or_else(|| "Failed to convert result to string".to_string())?;
                return Ok(result_str.to_rust_string_lossy(scope));
            }
        };

        // Poll the promise to completion with optional timeout
        if let Some(promise) = promise {
            self.poll_promise_with_timeout(promise, timeout_ms).await
        } else {
            Err("Unexpected state: promise was None".to_string())
        }
    }

    /// Poll a promise to completion with optional timeout.
    ///
    /// This method repeatedly performs microtask checkpoints and checks the promise state
    /// until it is settled or the timeout expires.
    async fn poll_promise_with_timeout(
        &mut self,
        promise: rusty_v8::Global<rusty_v8::Promise>,
        timeout_ms: Option<u64>,
    ) -> Result<String, String> {
        use tokio::time::Duration;

        let poll_future = async {
            loop {
                // Perform microtask checkpoint
                self.isolate.perform_microtask_checkpoint();

                // Poll the OpDriver for completed async operations
                let _ = self.poll_op_driver();

                // Check promise state
                let state = {
                    let scope = &mut rusty_v8::HandleScope::new(&mut self.isolate);
                    let context = rusty_v8::Local::new(scope, &self.primary_context);
                    let scope = &mut rusty_v8::ContextScope::new(scope, context);
                    let promise_local = rusty_v8::Local::new(scope, &promise);
                    promise_local.state()
                };

                match state {
                    rusty_v8::PromiseState::Fulfilled => {
                        // Promise resolved: extract result
                        let scope = &mut rusty_v8::HandleScope::new(&mut self.isolate);
                        let context = rusty_v8::Local::new(scope, &self.primary_context);
                        let scope = &mut rusty_v8::ContextScope::new(scope, context);
                        let promise_local = rusty_v8::Local::new(scope, &promise);
                        let result = promise_local.result(scope);
                        let result_str = result
                            .to_string(scope)
                            .ok_or_else(|| "Failed to convert result to string".to_string())?;
                        return Ok(result_str.to_rust_string_lossy(scope));
                    }
                    rusty_v8::PromiseState::Rejected => {
                        // Promise rejected: extract error
                        let scope = &mut rusty_v8::HandleScope::new(&mut self.isolate);
                        let context = rusty_v8::Local::new(scope, &self.primary_context);
                        let scope = &mut rusty_v8::ContextScope::new(scope, context);
                        let promise_local = rusty_v8::Local::new(scope, &promise);
                        let error = promise_local.result(scope);
                        let error_str = error
                            .to_string(scope)
                            .ok_or_else(|| "Failed to convert error to string".to_string())?;
                        return Err(format!(
                            "Promise rejected: {}",
                            error_str.to_rust_string_lossy(scope)
                        ));
                    }
                    rusty_v8::PromiseState::Pending => {
                        // Still pending: yield and try again
                        tokio::task::yield_now().await;
                    }
                }
            }
        };

        if let Some(timeout) = timeout_ms {
            tokio::time::timeout(Duration::from_millis(timeout), poll_future)
                .await
                .map_err(|_| {
                    self.isolate.terminate_execution();
                    format!("Execution timeout after {}ms", timeout)
                })?
        } else {
            poll_future.await
        }
    }

    /// Drain all pending microtasks.
    pub fn drain_tasks(&mut self) -> Result<(), String> {
        self.isolate.perform_microtask_checkpoint();
        Ok(())
    }

    /// Poll the OpDriver for ready operations and resolve them.
    ///
    /// This should be called after draining microtasks to handle completed async ops.
    pub fn poll_op_driver(&mut self) -> Result<(), String> {
        use futures::task::noop_waker;

        // Simple waker for polling
        let waker = noop_waker();
        let mut cx = Context::from_waker(&waker);

        // Poll for ready operations
        while let Poll::Ready((promise_id, _op_id, op_result)) =
            self.op_driver.borrow().poll_ready(&mut cx)
        {
            // Convert OpResult to Result
            let result = op_result.into_result();

            // Resolve the promise in V8
            let scope = &mut rusty_v8::HandleScope::new(&mut self.isolate);
            let context = rusty_v8::Local::new(scope, &self.primary_context);
            let scope = &mut rusty_v8::ContextScope::new(scope, context);

            if let Err(err) = resolve_op_in_scope(scope, promise_id, &result) {
                eprintln!("Failed to resolve op promise {}: {}", promise_id, err);
            }
        }

        Ok(())
    }

    /// Get the number of pending operations in the OpDriver.
    ///
    /// This is useful for debugging and monitoring async operation load.
    pub fn pending_ops(&self) -> usize {
        self.op_driver.borrow().len()
    }

    /// Get detailed statistics about in-flight operations.
    #[allow(dead_code)] // Will be exposed via API when async ops are fully wired
    pub fn op_driver_stats(&self) -> super::op_driver::OpInflightStats {
        self.op_driver.borrow().stats()
    }

    /// Shutdown the OpDriver, canceling any pending operations.
    ///
    /// This is called automatically when the runtime is dropped, but can
    /// be called explicitly to clean up resources earlier.
    pub fn shutdown_op_driver(&self) {
        self.op_driver.borrow().shutdown();
    }

    /// Install op bootstrap and host shims into the given context.
    fn bootstrap_ops_for_context(
        &mut self,
        context: &rusty_v8::Global<rusty_v8::Context>,
    ) -> Result<(), String> {
        let result = (|| -> Result<(), String> {
            let scope = &mut rusty_v8::HandleScope::new(&mut self.isolate);
            let context_local = rusty_v8::Local::new(scope, context);

            let registry_ptr = std::rc::Rc::into_raw(self.op_registry.clone());
            let driver_ptr = std::rc::Rc::into_raw(self.op_driver.clone());
            unsafe {
                context_local.set_aligned_pointer_in_embedder_data(
                    REGISTRY_EMBEDDER_SLOT,
                    registry_ptr as *mut c_void,
                );
                context_local.set_aligned_pointer_in_embedder_data(
                    DRIVER_EMBEDDER_SLOT,
                    driver_ptr as *mut c_void,
                );
            }

            let scope = &mut rusty_v8::ContextScope::new(scope, context_local);

            let code = rusty_v8::String::new(scope, OPS_BOOTSTRAP)
                .ok_or_else(|| "Failed to create ops bootstrap string".to_string())?;
            let script = rusty_v8::Script::compile(scope, code, None)
                .ok_or_else(|| "Failed to compile ops bootstrap".to_string())?;
            script
                .run(scope)
                .ok_or_else(|| "Failed to run ops bootstrap".to_string())?;

            let global = scope.get_current_context().global(scope);

            let sync_fn = rusty_v8::FunctionTemplate::new(scope, host_op_sync_callback)
                .get_function(scope)
                .ok_or_else(|| "Failed to create __host_op_sync__ function".to_string())?;
            let sync_name = rusty_v8::String::new(scope, "__host_op_sync__")
                .ok_or_else(|| "Failed to create sync function name".to_string())?;
            global
                .set(scope, sync_name.into(), sync_fn.into())
                .ok_or_else(|| "Failed to set __host_op_sync__".to_string())?;

            let async_impl_fn = rusty_v8::FunctionTemplate::new(scope, host_op_async_impl_callback)
                .get_function(scope)
                .ok_or_else(|| "Failed to create __host_op_async_impl__ function".to_string())?;
            let async_name = rusty_v8::String::new(scope, "__host_op_async_impl__")
                .ok_or_else(|| "Failed to create async function name".to_string())?;
            global
                .set(scope, async_name.into(), async_impl_fn.into())
                .ok_or_else(|| "Failed to set __host_op_async_impl__".to_string())?;

            Ok(())
        })();

        if result.is_err() {
            self.clear_context_registry_pointer(context);
        }

        result
    }

    /// Run the optional user-provided bootstrap script within the given context.
    fn run_user_bootstrap(
        &mut self,
        context: &rusty_v8::Global<rusty_v8::Context>,
    ) -> Result<(), String> {
        let script = match self.bootstrap_script.as_deref() {
            Some(script) => script,
            None => return Ok(()),
        };

        let scope = &mut rusty_v8::HandleScope::new(&mut self.isolate);
        let context_local = rusty_v8::Local::new(scope, context);
        let scope = &mut rusty_v8::ContextScope::new(scope, context_local);

        let code = rusty_v8::String::new(scope, script)
            .ok_or_else(|| "Failed to create bootstrap script string".to_string())?;
        let script = rusty_v8::Script::compile(scope, code, None)
            .ok_or_else(|| "Failed to compile bootstrap script".to_string())?;
        script
            .run(scope)
            .ok_or_else(|| "Failed to run bootstrap script".to_string())?;

        Ok(())
    }

    /// Clear the op registry pointer stored in the context's embedder data.
    fn clear_context_registry_pointer(&mut self, context: &rusty_v8::Global<rusty_v8::Context>) {
        let scope = &mut rusty_v8::HandleScope::new(&mut self.isolate);
        let context_local = rusty_v8::Local::new(scope, context);
        unsafe {
            // Clear registry pointer
            let registry_ptr =
                context_local.get_aligned_pointer_from_embedder_data(REGISTRY_EMBEDDER_SLOT);
            if !registry_ptr.is_null() {
                let _ = std::rc::Rc::from_raw(
                    registry_ptr as *const std::cell::RefCell<super::ops::OpRegistry>,
                );
                context_local.set_aligned_pointer_in_embedder_data(
                    REGISTRY_EMBEDDER_SLOT,
                    std::ptr::null_mut(),
                );
            }

            // Clear driver pointer
            let driver_ptr =
                context_local.get_aligned_pointer_from_embedder_data(DRIVER_EMBEDDER_SLOT);
            if !driver_ptr.is_null() {
                let _ = std::rc::Rc::from_raw(
                    driver_ptr
                        as *const std::cell::RefCell<super::op_driver::FuturesUnorderedDriver>,
                );
                context_local.set_aligned_pointer_in_embedder_data(
                    DRIVER_EMBEDDER_SLOT,
                    std::ptr::null_mut(),
                );
            }
        }
    }

    fn resolve_op_result(
        &mut self,
        promise_id: u32,
        op_result: &Result<serde_json::Value, String>,
    ) -> Result<(), String> {
        let scope = &mut rusty_v8::HandleScope::new(&mut self.isolate);
        let context_local = rusty_v8::Local::new(scope, &self.primary_context);
        let scope = &mut rusty_v8::ContextScope::new(scope, context_local);
        resolve_op_in_scope(scope, promise_id, op_result)
    }

    /// Create a new context and return its ID.
    fn create_context(&mut self) -> Result<usize, String> {
        let context_id = self.next_context_id;
        self.next_context_id += 1;

        let global_context = {
            let scope = &mut rusty_v8::HandleScope::new(&mut self.isolate);
            let context = rusty_v8::Context::new(scope, Default::default());
            rusty_v8::Global::new(scope, context)
        };

        self.bootstrap_ops_for_context(&global_context)?;
        if let Err(err) = self.run_user_bootstrap(&global_context) {
            self.clear_context_registry_pointer(&global_context);
            return Err(err);
        }

        self.contexts.insert(context_id, global_context);
        Ok(context_id)
    }

    /// Drop a context by ID.
    fn drop_context(&mut self, context_id: usize) {
        if let Some(context_global) = self.contexts.remove(&context_id) {
            self.clear_context_registry_pointer(&context_global);
        }
    }

    /// Evaluate code in a specific context.
    fn eval_in_context(&mut self, context_id: usize, code: &str) -> Result<String, String> {
        let context_global = self
            .contexts
            .get(&context_id)
            .ok_or_else(|| format!("Context {} not found", context_id))?;

        let scope = &mut rusty_v8::HandleScope::new(&mut self.isolate);
        let context = rusty_v8::Local::new(scope, context_global);
        let scope = &mut rusty_v8::ContextScope::new(scope, context);

        // Compile and run the code
        let code_v8 = rusty_v8::String::new(scope, code)
            .ok_or_else(|| "Failed to create V8 string".to_string())?;
        let script = rusty_v8::Script::compile(scope, code_v8, None)
            .ok_or_else(|| "Failed to compile script".to_string())?;
        let result = script
            .run(scope)
            .ok_or_else(|| "Failed to execute script".to_string())?;

        // Convert result to string
        let result_str = result.to_rust_string_lossy(scope);
        Ok(result_str)
    }

    /// Evaluate code asynchronously in a specific context with optional timeout.
    async fn eval_in_context_async(
        &mut self,
        context_id: usize,
        code: &str,
        timeout_ms: Option<u64>,
    ) -> Result<String, String> {
        let context_global = self
            .contexts
            .get(&context_id)
            .ok_or_else(|| format!("Context {} not found", context_id))?
            .clone(); // Clone the Global to avoid borrow issues

        // Compile and execute the script
        let promise = {
            let scope = &mut rusty_v8::HandleScope::new(&mut self.isolate);
            let context = rusty_v8::Local::new(scope, &context_global);
            let scope = &mut rusty_v8::ContextScope::new(scope, context);

            let code_v8 = rusty_v8::String::new(scope, code)
                .ok_or_else(|| "Failed to create V8 string".to_string())?;
            let script = rusty_v8::Script::compile(scope, code_v8, None)
                .ok_or_else(|| "Failed to compile script".to_string())?;
            let result = script
                .run(scope)
                .ok_or_else(|| "Failed to execute script".to_string())?;

            // Check if result is a promise
            if result.is_promise() {
                let promise: rusty_v8::Local<rusty_v8::Promise> = result
                    .try_into()
                    .map_err(|_| "Failed to cast promise result".to_string())?;
                Some(rusty_v8::Global::new(scope, promise))
            } else {
                // Non-promise result: convert immediately
                return Ok(result.to_rust_string_lossy(scope));
            }
        };

        // Poll the promise to completion with optional timeout
        if let Some(promise) = promise {
            self.poll_promise_in_context_with_timeout(promise, context_global, timeout_ms)
                .await
        } else {
            Err("Unexpected state: promise was None".to_string())
        }
    }

    /// Poll a promise to completion in a specific context with optional timeout.
    async fn poll_promise_in_context_with_timeout(
        &mut self,
        promise: rusty_v8::Global<rusty_v8::Promise>,
        context: rusty_v8::Global<rusty_v8::Context>,
        timeout_ms: Option<u64>,
    ) -> Result<String, String> {
        use tokio::time::Duration;

        let poll_future = async {
            loop {
                // Perform microtask checkpoint
                self.isolate.perform_microtask_checkpoint();

                // Poll the OpDriver for completed async operations
                let _ = self.poll_op_driver();

                // Check promise state
                let state = {
                    let scope = &mut rusty_v8::HandleScope::new(&mut self.isolate);
                    let context_local = rusty_v8::Local::new(scope, &context);
                    let scope = &mut rusty_v8::ContextScope::new(scope, context_local);
                    let promise_local = rusty_v8::Local::new(scope, &promise);
                    promise_local.state()
                };

                match state {
                    rusty_v8::PromiseState::Fulfilled => {
                        // Promise resolved: extract result
                        let scope = &mut rusty_v8::HandleScope::new(&mut self.isolate);
                        let context_local = rusty_v8::Local::new(scope, &context);
                        let scope = &mut rusty_v8::ContextScope::new(scope, context_local);
                        let promise_local = rusty_v8::Local::new(scope, &promise);
                        let result = promise_local.result(scope);
                        let result_str = result
                            .to_string(scope)
                            .ok_or_else(|| "Failed to convert result to string".to_string())?;
                        return Ok(result_str.to_rust_string_lossy(scope));
                    }
                    rusty_v8::PromiseState::Rejected => {
                        // Promise rejected: extract error
                        let scope = &mut rusty_v8::HandleScope::new(&mut self.isolate);
                        let context_local = rusty_v8::Local::new(scope, &context);
                        let scope = &mut rusty_v8::ContextScope::new(scope, context_local);
                        let promise_local = rusty_v8::Local::new(scope, &promise);
                        let error = promise_local.result(scope);
                        let error_str = error
                            .to_string(scope)
                            .ok_or_else(|| "Failed to convert error to string".to_string())?;
                        return Err(format!(
                            "Promise rejected: {}",
                            error_str.to_rust_string_lossy(scope)
                        ));
                    }
                    rusty_v8::PromiseState::Pending => {
                        // Still pending: yield and continue
                        tokio::task::yield_now().await;
                    }
                }
            }
        };

        // Run with optional timeout
        if let Some(timeout) = timeout_ms {
            tokio::time::timeout(Duration::from_millis(timeout), poll_future)
                .await
                .map_err(|_| {
                    self.isolate.terminate_execution();
                    "Execution timeout exceeded".to_string()
                })?
        } else {
            poll_future.await
        }
    }
}

impl Drop for JsRuntimeCore {
    fn drop(&mut self) {
        // Drop registry pointers for all secondary contexts
        let drained_contexts: Vec<_> = self.contexts.drain().map(|(_, context)| context).collect();
        for context in drained_contexts {
            self.clear_context_registry_pointer(&context);
        }

        // Drop the registry pointer for the primary context
        let primary_context = self.primary_context.clone();
        self.clear_context_registry_pointer(&primary_context);
    }
}

/// Spawn a runtime thread and return a command sender.
pub fn spawn_runtime_thread(
    config: RuntimeConfig,
) -> Result<async_mpsc::UnboundedSender<HostCommand>, String> {
    let (tx, mut rx) = async_mpsc::unbounded_channel::<HostCommand>();

    // Spawn OS thread
    thread::Builder::new()
        .name("v8-runtime".to_string())
        .spawn(move || {
            // Build single-thread Tokio runtime
            let tokio_rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("Failed to create Tokio runtime");

            // Run the async event loop
            tokio_rt.block_on(async move {
                // Construct JsRuntimeCore
                let mut runtime = match JsRuntimeCore::new(&config) {
                    Ok(rt) => rt,
                    Err(e) => {
                        eprintln!("Failed to create runtime: {}", e);
                        return;
                    }
                };

                // Event loop: await mailbox commands
                while let Some(cmd) = rx.recv().await {
                    match cmd {
                        HostCommand::Eval { code, responder } => {
                            let result = runtime.eval(&code);
                            let _ = responder.send(result);
                        }
                        HostCommand::EvalAsync {
                            code,
                            timeout_ms,
                            responder,
                        } => {
                            let result = runtime.eval_async(&code, timeout_ms).await;
                            let _ = responder.send(result);
                        }
                        HostCommand::ExecuteModule { url, responder } => {
                            // Placeholder
                            let _ = responder.send(Err(format!(
                                "Module execution not yet implemented: {}",
                                url
                            )));
                        }
                        HostCommand::DrainTasks { responder } => {
                            let result = runtime.drain_tasks();
                            // Poll the OpDriver after draining microtasks
                            if result.is_ok() {
                                let _ = runtime.poll_op_driver();
                            }
                            let _ = responder.send(result);
                        }
                        HostCommand::OpCall {
                            op_id,
                            promise_id,
                            args,
                            responder,
                        } => {
                            // Call the op and resolve the associated promise in JavaScript
                            let op_result = runtime.op_registry.borrow().call_op(op_id, args);
                            let resolve_status = runtime.resolve_op_result(promise_id, &op_result);

                            let final_result = match resolve_status {
                                Ok(_) => op_result,
                                Err(resolve_err) => Err(resolve_err),
                            };

                            let _ = responder.send(match &final_result {
                                Ok(value) => Ok(value.clone()),
                                Err(err) => Err(err.clone()),
                            });
                        }
                        HostCommand::RegisterOp {
                            name,
                            mode,
                            permissions,
                            handler,
                            responder,
                        } => {
                            let result = runtime.op_registry.borrow_mut().register_op(
                                name,
                                mode,
                                permissions,
                                handler,
                            );
                            let _ = responder.send(result);
                        }
                        HostCommand::CreateContext { responder } => {
                            let result = runtime.create_context();
                            let _ = responder.send(result);
                        }
                        HostCommand::DropContext { context_id } => {
                            runtime.drop_context(context_id);
                        }
                        HostCommand::EvalInContext {
                            context_id,
                            code,
                            responder,
                        } => {
                            let result = runtime.eval_in_context(context_id, &code);
                            let _ = responder.send(result);
                        }
                        HostCommand::EvalInContextAsync {
                            context_id,
                            code,
                            timeout_ms,
                            responder,
                        } => {
                            let result = runtime
                                .eval_in_context_async(context_id, &code, timeout_ms)
                                .await;
                            let _ = responder.send(result);
                        }
                        HostCommand::GetPendingOps { responder } => {
                            let count = runtime.pending_ops();
                            let _ = responder.send(count);
                        }
                        HostCommand::Shutdown => {
                            // Shutdown the OpDriver before exiting
                            runtime.shutdown_op_driver();
                            break;
                        }
                    }
                }
            });
        })
        .map_err(|e| format!("Failed to spawn runtime thread: {}", e))?;

    Ok(tx)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::config::Permission as ConfigPermission;
    use crate::runtime::ops::{self, OpMode, Permission as OpPermission};
    use serde_json::json;
    use std::sync::Arc;
    use std::time::Duration;

    #[test]
    fn test_runtime_core_creation() {
        let config = RuntimeConfig::default();
        let runtime = JsRuntimeCore::new(&config);
        assert!(runtime.is_ok());
    }

    #[test]
    fn test_runtime_core_eval() {
        let config = RuntimeConfig::default();
        let mut runtime = JsRuntimeCore::new(&config).unwrap();

        let result = runtime.eval("1 + 1");
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "2");
    }

    #[test]
    fn test_runtime_core_with_bootstrap() {
        let config =
            RuntimeConfig::new().with_bootstrap("globalThis.bootstrapped = true;".to_string());
        let mut runtime = JsRuntimeCore::new(&config).unwrap();

        let result = runtime.eval("globalThis.bootstrapped");
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "true");
    }

    #[test]
    fn test_spawn_runtime_thread() {
        let config = RuntimeConfig::default();
        let tx = spawn_runtime_thread(config).unwrap();

        // Send eval command
        let (result_tx, result_rx) = mpsc::channel();
        tx.send(HostCommand::Eval {
            code: "2 + 2".to_string(),
            responder: result_tx,
        })
        .unwrap();

        // Wait for result
        let result = result_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("Timeout waiting for result");

        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "4");

        // Shutdown
        tx.send(HostCommand::Shutdown).unwrap();

        // Give the thread time to shutdown
        thread::sleep(Duration::from_millis(100));
    }

    #[tokio::test]
    async fn test_eval_async_with_promise() {
        let config = RuntimeConfig::default();
        let mut runtime = JsRuntimeCore::new(&config).unwrap();

        // Test async code that returns a resolved promise
        let code = "Promise.resolve(42)";
        let result = runtime.eval_async(code, None).await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "42");
    }

    #[tokio::test]
    async fn test_eval_async_without_promise() {
        let config = RuntimeConfig::default();
        let mut runtime = JsRuntimeCore::new(&config).unwrap();

        // Test sync code (non-promise)
        let code = "10 + 20";
        let result = runtime.eval_async(code, None).await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "30");
    }

    #[tokio::test]
    async fn test_eval_async_with_delayed_promise() {
        let config = RuntimeConfig::default();
        let mut runtime = JsRuntimeCore::new(&config).unwrap();

        // Test async code with queueMicrotask
        let code = r#"
            new Promise((resolve) => {
                queueMicrotask(() => resolve('async result'));
            })
        "#;
        let result = runtime.eval_async(code, None).await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "async result");
    }

    #[tokio::test]
    async fn test_eval_async_with_rejected_promise() {
        let config = RuntimeConfig::default();
        let mut runtime = JsRuntimeCore::new(&config).unwrap();

        // Test promise rejection
        let code = "Promise.reject(new Error('test error'))";
        let result = runtime.eval_async(code, None).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Promise rejected"));
    }

    #[tokio::test]
    async fn test_eval_async_with_timeout() {
        let config = RuntimeConfig::default();
        let mut runtime = JsRuntimeCore::new(&config).unwrap();

        // Test timeout with a promise that never resolves
        let code = "new Promise(() => {})";
        let result = runtime.eval_async(code, Some(100)).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("timeout"));
    }

    #[tokio::test]
    async fn test_eval_async_completes_before_timeout() {
        let config = RuntimeConfig::default();
        let mut runtime = JsRuntimeCore::new(&config).unwrap();

        // Test that fast promises complete before timeout
        let code = "Promise.resolve('quick')";
        let result = runtime.eval_async(code, Some(1000)).await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "quick");
    }

    #[test]
    fn test_drain_tasks() {
        let config = RuntimeConfig::default();
        let mut runtime = JsRuntimeCore::new(&config).unwrap();

        // Queue some microtasks
        let code = "queueMicrotask(() => { globalThis.flag = true; })";
        runtime.eval(code).unwrap();

        // Drain tasks
        runtime.drain_tasks().unwrap();

        // Verify microtask ran
        let result = runtime.eval("globalThis.flag");
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "true");
    }

    #[test]
    fn test_eval_async_via_command() {
        let config = RuntimeConfig::default();
        let tx = spawn_runtime_thread(config).unwrap();

        // Send async eval command with promise
        let (result_tx, result_rx) = mpsc::channel();
        tx.send(HostCommand::EvalAsync {
            code: "Promise.resolve(100)".to_string(),
            timeout_ms: None,
            responder: result_tx,
        })
        .unwrap();

        // Wait for result
        let result = result_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("Timeout waiting for result");

        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "100");

        // Shutdown
        tx.send(HostCommand::Shutdown).unwrap();
        thread::sleep(Duration::from_millis(100));
    }

    #[test]
    fn test_eval_async_timeout_via_command() {
        let config = RuntimeConfig::default();
        let tx = spawn_runtime_thread(config).unwrap();

        // Send async eval command with timeout
        let (result_tx, result_rx) = mpsc::channel();
        tx.send(HostCommand::EvalAsync {
            code: "new Promise(() => {})".to_string(),
            timeout_ms: Some(100),
            responder: result_tx,
        })
        .unwrap();

        // Wait for result
        let result = result_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("Timeout waiting for result");

        assert!(result.is_err());
        assert!(result.unwrap_err().contains("timeout"));

        // Shutdown
        tx.send(HostCommand::Shutdown).unwrap();
        thread::sleep(Duration::from_millis(100));
    }

    #[test]
    fn test_runtime_core_applies_permissions() {
        let config = RuntimeConfig::new().with_permission(ConfigPermission::Timers);
        let runtime = JsRuntimeCore::new(&config).unwrap();

        assert!(runtime
            .op_registry
            .borrow()
            .has_permission(&OpPermission::Timers));
    }

    #[test]
    fn test_drain_tasks_via_command_channel() {
        let config = RuntimeConfig::default();
        let tx = spawn_runtime_thread(config).unwrap();

        let (result_tx, result_rx) = mpsc::channel();
        tx.send(HostCommand::DrainTasks {
            responder: result_tx,
        })
        .unwrap();

        let result = result_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("Timeout waiting for drain result");
        assert!(result.is_ok());

        tx.send(HostCommand::Shutdown).unwrap();
        thread::sleep(Duration::from_millis(100));
    }

    #[test]
    fn test_execute_module_command_returns_placeholder_error() {
        let config = RuntimeConfig::default();
        let tx = spawn_runtime_thread(config).unwrap();

        let (result_tx, result_rx) = mpsc::channel();
        tx.send(HostCommand::ExecuteModule {
            url: "module.js".to_string(),
            responder: result_tx,
        })
        .unwrap();

        let result = result_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("Timeout waiting for module result");
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .contains("Module execution not yet implemented"));

        tx.send(HostCommand::Shutdown).unwrap();
        thread::sleep(Duration::from_millis(100));
    }

    #[test]
    fn test_register_op_and_call_via_command() {
        let config = RuntimeConfig::default();
        let tx = spawn_runtime_thread(config).unwrap();

        let (register_tx, register_rx) = mpsc::channel();
        let handler: ops::OpHandler = Arc::new(|_| Ok(json!({ "status": "ok" })));

        tx.send(HostCommand::RegisterOp {
            name: "cmd_op".to_string(),
            mode: OpMode::Sync,
            permissions: vec![],
            handler,
            responder: register_tx,
        })
        .unwrap();

        let op_id = register_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("Timeout waiting for op registration")
            .expect("Op registration failed");

        let (call_tx, call_rx) = mpsc::channel();
        tx.send(HostCommand::OpCall {
            op_id,
            promise_id: 123,
            args: vec![],
            responder: call_tx,
        })
        .unwrap();

        let result = call_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("Timeout waiting for op call result");
        // Without an outstanding promise the resolution fails, but the handler still runs.
        assert!(result.is_err());

        tx.send(HostCommand::Shutdown).unwrap();
        thread::sleep(Duration::from_millis(100));
    }

    #[test]
    fn test_resolve_op_result_success() {
        let config = RuntimeConfig::default();
        let mut runtime = JsRuntimeCore::new(&config).unwrap();

        runtime
            .eval(
                r#"
                globalThis.__host_op_async_impl__ = function(opId, promiseId, payload) {
                    globalThis.__capturedPromiseId = promiseId;
                };
                globalThis.promiseResult = undefined;
                globalThis.pendingPromise = __host_op_async__(1, { foo: 'bar' });
                globalThis.pendingPromise.then(value => { globalThis.promiseResult = value; });
            "#,
            )
            .unwrap();

        let promise_id: u32 = runtime
            .eval("globalThis.__capturedPromiseId")
            .unwrap()
            .parse()
            .unwrap();

        runtime
            .resolve_op_result(promise_id, &Ok(json!({ "foo": "bar" })))
            .unwrap();

        runtime.drain_tasks().unwrap();

        let result = runtime
            .eval("globalThis.promiseResult && globalThis.promiseResult.foo")
            .unwrap();
        assert_eq!(result, "bar");
    }

    #[test]
    fn test_resolve_op_result_error() {
        let config = RuntimeConfig::default();
        let mut runtime = JsRuntimeCore::new(&config).unwrap();

        runtime
            .eval(
                r#"
                globalThis.__host_op_async_impl__ = function(opId, promiseId) {
                    globalThis.__capturedErrorPromiseId = promiseId;
                };
                globalThis.promiseErrorMessage = undefined;
                globalThis.errorPromise = __host_op_async__(2);
                globalThis.errorPromise.catch(err => {
                    globalThis.promiseErrorMessage = err && err.message;
                });
            "#,
            )
            .unwrap();

        let promise_id: u32 = runtime
            .eval("globalThis.__capturedErrorPromiseId")
            .unwrap()
            .parse()
            .unwrap();

        runtime
            .resolve_op_result(promise_id, &Err("boom".to_string()))
            .unwrap();

        runtime.drain_tasks().unwrap();

        let error_message = runtime.eval("globalThis.promiseErrorMessage").unwrap();
        assert_eq!(error_message, "boom");
    }

    // Step 0: Baseline tests capturing current async op behavior before OpDriver integration

    #[tokio::test]
    async fn test_async_op_resolves_inline() {
        // Captures current behavior: async ops resolve immediately via host_op_async_impl_callback
        let config = RuntimeConfig::default();
        let tx = spawn_runtime_thread(config).unwrap();

        // Register an async op via command
        let (register_tx, register_rx) = mpsc::channel();
        let handler: ops::OpHandler = Arc::new(|args: Vec<serde_json::Value>| {
            Ok(json!({ "result": args[0].as_i64().unwrap() * 2 }))
        });

        tx.send(HostCommand::RegisterOp {
            name: "double".to_string(),
            mode: OpMode::Async,
            permissions: vec![],
            handler,
            responder: register_tx,
        })
        .unwrap();

        let _op_id = register_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("Timeout waiting for op registration")
            .expect("Op registration failed");

        // Call the async op and verify it resolves inline
        let (result_tx, result_rx) = mpsc::channel();
        tx.send(HostCommand::EvalAsync {
            code: r#"
                (async () => {
                    globalThis.asyncResult = null;
                    __host_op_async__(0, 21).then(result => {
                        globalThis.asyncResult = result;
                    });
                    // Allow microtasks to run
                    await Promise.resolve();
                    return JSON.stringify(globalThis.asyncResult);
                })()
            "#
            .to_string(),
            timeout_ms: None,
            responder: result_tx,
        })
        .unwrap();

        let result = result_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("Timeout waiting for result")
            .expect("Eval failed");

        let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();
        assert_eq!(parsed["result"], 42);

        // Shutdown
        tx.send(HostCommand::Shutdown).unwrap();
        thread::sleep(Duration::from_millis(100));
    }

    #[test]
    fn test_promise_stats_available() {
        // Verify __getPromiseStats is available for observability
        let config = RuntimeConfig::default();
        let mut runtime = JsRuntimeCore::new(&config).unwrap();

        let result = runtime
            .eval(
                r#"
            const stats = __getPromiseStats();
            JSON.stringify({
                hasNextId: typeof stats.nextPromiseId === 'number',
                hasRingSize: typeof stats.ringSize === 'number',
                hasMapSize: typeof stats.mapSize === 'number'
            })
            "#,
            )
            .unwrap();

        let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();
        assert_eq!(parsed["hasNextId"], true);
        assert_eq!(parsed["hasRingSize"], true);
        assert_eq!(parsed["hasMapSize"], true);
    }
}

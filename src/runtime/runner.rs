//! Runtime thread backed by `deno_core::JsRuntime`.
//!
//! This module hosts the JavaScript engine on a dedicated OS thread with a
//! single-threaded Tokio runtime. Commands from Python are forwarded through
//! [`RuntimeCommand`] and executed sequentially on that thread.

use crate::runtime::config::RuntimeConfig;
use crate::runtime::ops::{python_extension, PythonOpMode, PythonOpRegistry};
use deno_core::error::CoreError;
use deno_core::{JsRuntime, PollEventLoopOptions, RuntimeOptions};
use pyo3::prelude::Py;
use pyo3::PyAny;
use pyo3_async_runtimes::TaskLocals;
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

/// Commands sent to the runtime thread.
pub enum RuntimeCommand {
    Eval {
        code: String,
        responder: Sender<Result<String, String>>,
    },
    EvalAsync {
        code: String,
        timeout_ms: Option<u64>,
        task_locals: Option<TaskLocals>,
        responder: oneshot::Sender<Result<String, String>>,
    },
    RegisterPythonOp {
        name: String,
        mode: PythonOpMode,
        handler: Py<PyAny>,
        responder: Sender<Result<u32, String>>,
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
    task_locals: Option<TaskLocals>,
}

impl RuntimeCore {
    fn new(config: RuntimeConfig) -> Result<Self, String> {
        let registry = PythonOpRegistry::new();
        let extension = python_extension(registry.clone());

        let mut js_runtime = JsRuntime::new(RuntimeOptions {
            extensions: vec![extension],
            ..Default::default()
        });

        if let Some(script) = config.bootstrap_script {
            js_runtime
                .execute_script("<bootstrap>", script)
                .map_err(|err| err.to_string())?;
        }

        Ok(Self {
            js_runtime,
            registry,
            task_locals: None,
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
                RuntimeCommand::Shutdown { responder } => {
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

    fn eval_sync(&mut self, code: &str) -> Result<String, String> {
        let global_value = self
            .js_runtime
            .execute_script("<eval>", code.to_string())
            .map_err(|err| err.to_string())?;
        self.value_to_string(global_value)
    }

    async fn eval_async(
        &mut self,
        code: String,
        timeout_ms: Option<u64>,
    ) -> Result<String, String> {
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

        self.value_to_string(resolved)
    }

    fn value_to_string(
        &mut self,
        value: deno_core::v8::Global<deno_core::v8::Value>,
    ) -> Result<String, String> {
        let scope = &mut self.js_runtime.handle_scope();
        let local = deno_core::v8::Local::new(scope, value);
        let string_value = local
            .to_string(scope)
            .ok_or_else(|| "Unable to convert value to string".to_string())?;
        Ok(string_value.to_rust_string_lossy(scope))
    }
}

fn to_string(error: CoreError) -> String {
    error.to_string()
}

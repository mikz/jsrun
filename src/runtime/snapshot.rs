//! Snapshot builder utilities built on top of `deno_core::JsRuntimeForSnapshot`.

use crate::runtime::error::{JsExceptionDetails, RuntimeError, RuntimeResult};
use deno_core::error::CoreError;
use deno_core::JsRuntimeForSnapshot;
use deno_core::RuntimeOptions;

#[derive(Debug, Clone)]
pub struct SnapshotBuilderConfig {
    pub bootstrap_script: Option<String>,
    pub enable_console: Option<bool>,
}

impl Default for SnapshotBuilderConfig {
    fn default() -> Self {
        Self {
            bootstrap_script: None,
            enable_console: Some(false),
        }
    }
}

pub struct SnapshotBuilder {
    runtime: Option<JsRuntimeForSnapshot>,
}

impl SnapshotBuilder {
    pub fn new(config: SnapshotBuilderConfig) -> RuntimeResult<Self> {
        let mut runtime = create_runtime().map_err(|err| {
            RuntimeError::internal(format!("Failed to initialize snapshot runtime: {err}"))
        })?;

        if config.enable_console == Some(false) {
            disable_console(&mut runtime)?;
        }

        if let Some(script) = config.bootstrap_script {
            execute_script(&mut runtime, "<bootstrap>", &script)?;
        }

        Ok(Self {
            runtime: Some(runtime),
        })
    }

    pub fn execute_script(&mut self, name: &str, source: &str) -> RuntimeResult<()> {
        let runtime = self
            .runtime
            .as_mut()
            .ok_or_else(|| RuntimeError::internal("Snapshot has already been built"))?;
        execute_script(runtime, name, source)
    }

    pub fn build(mut self) -> RuntimeResult<Vec<u8>> {
        let runtime = self
            .runtime
            .take()
            .ok_or_else(|| RuntimeError::internal("Snapshot has already been built"))?;
        Ok(runtime.snapshot().into_vec())
    }
}

fn create_runtime() -> Result<JsRuntimeForSnapshot, CoreError> {
    JsRuntimeForSnapshot::try_new(RuntimeOptions {
        is_main: true,
        ..Default::default()
    })
}

fn disable_console(runtime: &mut JsRuntimeForSnapshot) -> RuntimeResult<()> {
    execute_script(
        runtime,
        "<disable_console>",
        r#"
        (() => {
            const noop = () => {};
            const stub = new Proxy(Object.create(null), { get: () => noop });
            const existing = globalThis.console;
            if (typeof existing === "object" && existing !== null) {
                for (const key of Reflect.ownKeys(existing)) {
                    try { existing[key] = noop; } catch (_) {}
                }
                return;
            }
            globalThis.console = stub;
        })();
        "#,
    )
}

fn execute_script(
    runtime: &mut JsRuntimeForSnapshot,
    name: &str,
    source: &str,
) -> RuntimeResult<()> {
    runtime
        .execute_script(name.to_string(), source.to_string())
        .map(|_| ())
        .map_err(|err| RuntimeError::javascript(JsExceptionDetails::from_js_error(*err)))
}

//! Runtime context - lightweight handle for namespace-isolated execution.

use super::RuntimeHandle;
use pyo3::exceptions::PyRuntimeError;
use pyo3::prelude::*;

/// Lightweight context handle for namespace-isolated execution.
///
/// Each context has its own global object and maintains independent state.
/// All operations are delegated to the runtime thread via message passing.
///
/// Example:
/// ```python
/// import v8
///
/// runtime = v8.Runtime.spawn()
/// ctx1 = runtime.create_context()
/// ctx2 = runtime.create_context()
///
/// ctx1.eval("var x = 10")
/// ctx2.eval("var x = 20")
///
/// print(ctx1.eval("x"))  # "10"
/// print(ctx2.eval("x"))  # "20"
///
/// ctx1.close()
/// ctx2.close()
/// runtime.close()
/// ```
#[pyclass(unsendable)]
pub struct RuntimeContext {
    context_id: usize,
    runtime_handle: RuntimeHandle,
    closed: bool,
}

impl RuntimeContext {
    /// Create a new runtime context.
    ///
    /// This is called internally by Runtime.create_context(), not by users directly.
    pub(crate) fn new(context_id: usize, runtime_handle: RuntimeHandle) -> Self {
        Self {
            context_id,
            runtime_handle,
            closed: false,
        }
    }
}

#[pymethods]
impl RuntimeContext {
    /// Evaluate JavaScript code synchronously in this context.
    ///
    /// Args:
    ///     code (str): JavaScript code to evaluate
    ///
    /// Returns:
    ///     str: String representation of the result
    ///
    /// Raises:
    ///     RuntimeError: If evaluation fails or context is closed
    fn eval(&self, py: Python<'_>, code: &str) -> PyResult<String> {
        if self.closed {
            return Err(PyRuntimeError::new_err("Context has been closed"));
        }

        let code_owned = code.to_owned();
        let context_id = self.context_id;
        let handle = self.runtime_handle.clone();

        py.detach(move || handle.eval_in_context(context_id, &code_owned))
            .map_err(|e| PyRuntimeError::new_err(format!("Evaluation failed: {}", e)))
    }

    /// Evaluate JavaScript code asynchronously in this context.
    ///
    /// Args:
    ///     code (str): JavaScript code to evaluate
    ///     timeout_ms (int | None): Optional timeout in milliseconds
    ///
    /// Returns:
    ///     str: String representation of the result
    ///
    /// Raises:
    ///     RuntimeError: If evaluation fails, times out, or context is closed
    #[pyo3(signature = (code, /, *, timeout_ms=None))]
    fn eval_async<'py>(
        &self,
        py: Python<'py>,
        code: String,
        timeout_ms: Option<u64>,
    ) -> PyResult<Bound<'py, PyAny>> {
        if self.closed {
            return Err(PyRuntimeError::new_err("Context has been closed"));
        }

        let context_id = self.context_id;
        let handle = self.runtime_handle.clone();

        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            handle
                .eval_in_context_async(context_id, &code, timeout_ms)
                .await
                .map_err(|e| PyRuntimeError::new_err(format!("Evaluation failed: {}", e)))
        })
    }

    /// Check if the context has been closed.
    fn is_closed(&self) -> bool {
        self.closed || self.runtime_handle.is_shutdown()
    }

    /// Close the context, releasing associated V8 resources.
    ///
    /// After closing, this context cannot be used for evaluation.
    /// This method is idempotent.
    fn close(&mut self) -> PyResult<()> {
        if self.closed {
            return Ok(());
        }

        // If the runtime is already shut down, treat the context as closed.
        if self.runtime_handle.is_shutdown() {
            self.closed = true;
            return Ok(());
        }

        match self.runtime_handle.drop_context(self.context_id) {
            Ok(()) => {
                self.closed = true;
                Ok(())
            }
            Err(e) => {
                // During runtime shutdown the command channel may already be closed.
                // Treat this as a clean close instead of surfacing an error.
                if self.runtime_handle.is_shutdown() {
                    self.closed = true;
                    Ok(())
                } else {
                    Err(PyRuntimeError::new_err(format!(
                        "Failed to close context: {}",
                        e
                    )))
                }
            }
        }
    }

    /// Context manager support: __enter__
    fn __enter__(slf: PyRef<'_, Self>) -> PyRef<'_, Self> {
        slf
    }

    /// Context manager support: __exit__
    fn __exit__(
        &mut self,
        _exc_type: Option<&Bound<'_, PyAny>>,
        _exc_value: Option<&Bound<'_, PyAny>>,
        _traceback: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<bool> {
        self.close()?;
        Ok(false) // Don't suppress exceptions
    }
}

impl Drop for RuntimeContext {
    fn drop(&mut self) {
        if self.closed {
            return;
        }

        if self.runtime_handle.is_shutdown() {
            self.closed = true;
            return;
        }

        // Best-effort cleanup; ignore errors during shutdown.
        let _ = self.runtime_handle.drop_context(self.context_id);
        self.closed = true;
    }
}

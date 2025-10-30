//! Python bindings for the Tokio-based JavaScript runtime.
//!
//! This module provides the `Runtime` class that Python code uses to spawn
//! and interact with JavaScript runtimes.

use super::{initialize_platform_once, RuntimeConfig, RuntimeHandle};
use pyo3::exceptions::PyRuntimeError;
use pyo3::prelude::*;
use std::cell::RefCell;

/// Tokio-based JavaScript runtime.
///
/// Each runtime owns a single V8 isolate running on a dedicated OS thread
/// with a Tokio event loop. This is the new async-first runtime architecture
/// replacing the synchronous Isolate-based API.
///
/// Example:
/// ```python
/// import v8
///
/// # Spawn a new runtime
/// runtime = v8.Runtime.spawn()
///
/// # Evaluate JavaScript code
/// result = runtime.eval("2 + 2")
/// print(result)  # "4"
///
/// # Clean shutdown
/// runtime.close()
/// ```
#[pyclass(unsendable)]
pub struct Runtime {
    handle: RefCell<Option<RuntimeHandle>>,
}

#[pymethods]
impl Runtime {
    /// Spawn a new JavaScript runtime with default configuration.
    ///
    /// Returns:
    ///     Runtime: A new runtime instance
    ///
    /// Raises:
    ///     RuntimeError: If runtime creation fails
    #[staticmethod]
    fn spawn() -> PyResult<Self> {
        // Initialize V8 platform if needed
        initialize_platform_once();

        // Create runtime with default config
        let config = RuntimeConfig::default();
        let handle = RuntimeHandle::spawn(config)
            .map_err(|e| PyRuntimeError::new_err(format!("Failed to spawn runtime: {}", e)))?;

        Ok(Self {
            handle: RefCell::new(Some(handle)),
        })
    }

    /// Evaluate JavaScript code asynchronously with optional timeout.
    ///
    /// This method supports promises and async JavaScript code.
    ///
    /// Args:
    ///     code (str): JavaScript code to evaluate
    ///     timeout_ms (int | None): Optional timeout in milliseconds
    ///
    /// Returns:
    ///     str: String representation of the result
    ///
    /// Raises:
    ///     RuntimeError: If evaluation fails, times out, or runtime is closed
    ///
    /// Example:
    ///     ```python
    ///     import v8
    ///     import asyncio
    ///
    ///     async def main():
    ///         runtime = v8.Runtime.spawn()
    ///         result = await runtime.eval_async("Promise.resolve(42)")
    ///         print(result)  # "42"
    ///         runtime.close()
    ///
    ///     asyncio.run(main())
    ///     ```
    #[pyo3(signature = (code, /, *, timeout_ms=None))]
    fn eval_async<'py>(
        &self,
        py: Python<'py>,
        code: String,
        timeout_ms: Option<u64>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let handle_opt = self.handle.borrow();
        let handle = handle_opt
            .as_ref()
            .ok_or_else(|| PyRuntimeError::new_err("Runtime has been closed"))?;

        // Clone the handle for the async task
        let handle_clone = handle.clone();

        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            handle_clone
                .eval_async(&code, timeout_ms)
                .await
                .map_err(|e| PyRuntimeError::new_err(format!("Evaluation failed: {}", e)))
        })
    }

    /// Evaluate JavaScript code synchronously (blocking).
    ///
    /// This is a convenience method that blocks until the evaluation completes.
    /// For async code, consider using `eval_async` instead.
    ///
    /// Args:
    ///     code (str): JavaScript code to evaluate
    ///
    /// Returns:
    ///     str: String representation of the result
    ///
    /// Raises:
    ///     RuntimeError: If evaluation fails or runtime is closed
    fn eval(&self, py: Python<'_>, code: &str) -> PyResult<String> {
        let handle_opt = self.handle.borrow();
        let handle = handle_opt
            .as_ref()
            .ok_or_else(|| PyRuntimeError::new_err("Runtime has been closed"))?;

        let handle_clone = handle.clone();
        let code_owned = code.to_owned();
        py.detach(move || handle_clone.eval_sync(&code_owned))
            .map_err(|e| PyRuntimeError::new_err(format!("Evaluation failed: {}", e)))
    }

    /// Check if the runtime has been shut down.
    ///
    /// Returns:
    ///     bool: True if the runtime has been closed, False otherwise
    fn is_closed(&self) -> bool {
        let handle_opt = self.handle.borrow();
        match handle_opt.as_ref() {
            Some(handle) => handle.is_shutdown(),
            None => true,
        }
    }

    /// Shut down the runtime gracefully.
    ///
    /// After calling this method, the runtime cannot be used for evaluation.
    /// This method is idempotent and safe to call multiple times.
    ///
    /// Raises:
    ///     RuntimeError: If shutdown fails
    fn close(&self) -> PyResult<()> {
        let mut handle_opt = self.handle.borrow_mut();
        if let Some(mut handle) = handle_opt.take() {
            handle
                .close()
                .map_err(|e| PyRuntimeError::new_err(format!("Shutdown failed: {}", e)))?;
        }
        Ok(())
    }

    /// Get the number of pending operations in the OpDriver.
    ///
    /// Returns 0 if the runtime has been shut down or if there are no pending operations.
    /// This is useful for monitoring async operation load and debugging.
    ///
    /// Returns:
    ///     int: Number of pending async operations
    fn pending_ops(&self) -> PyResult<usize> {
        let handle_opt = self.handle.borrow();
        match handle_opt.as_ref() {
            Some(handle) => Ok(handle.pending_ops()),
            None => Ok(0),
        }
    }

    /// Context manager support: __enter__
    fn __enter__(slf: PyRef<'_, Self>) -> PyRef<'_, Self> {
        slf
    }

    /// Context manager support: __exit__
    fn __exit__(
        &self,
        _exc_type: Option<&Bound<'_, PyAny>>,
        _exc_value: Option<&Bound<'_, PyAny>>,
        _traceback: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<bool> {
        self.close()?;
        Ok(false) // Don't suppress exceptions
    }

    /// Register a Python callable as an op.
    ///
    /// Args:
    ///     name (str): Op name for identification
    ///     handler (Callable): Python function to call when op is invoked
    ///     mode (str): "sync" or "async" (default: "sync")
    ///     permissions (List[str]): Required permissions (default: [])
    ///
    /// Returns:
    ///     int: Op ID for this operation
    ///
    /// Example:
    ///     ```python
    ///     def my_op(x, y):
    ///         return x + y
    ///
    ///     op_id = runtime.register_op("add", my_op, mode="sync")
    ///
    ///     # Call from JavaScript:
    ///     # const result = __host_op_sync__(op_id, 10, 20)
    ///     ```
    #[pyo3(signature = (name, handler, /, *, mode="sync", permissions=vec![]))]
    fn register_op(
        &self,
        py: Python<'_>,
        name: String,
        handler: Py<PyAny>,
        mode: &str,
        permissions: Vec<String>,
    ) -> PyResult<u32> {
        let handle_opt = self.handle.borrow();
        let handle = handle_opt
            .as_ref()
            .ok_or_else(|| PyRuntimeError::new_err("Runtime has been closed"))?;

        // Parse mode
        let op_mode = match mode {
            "sync" => super::ops::OpMode::Sync,
            "async" => super::ops::OpMode::Async,
            _ => {
                return Err(PyRuntimeError::new_err(format!(
                    "Invalid mode '{}', expected 'sync' or 'async'",
                    mode
                )))
            }
        };

        // Parse permissions
        let op_permissions = permissions
            .iter()
            .map(|p| parse_permission(p))
            .collect::<PyResult<Vec<_>>>()?;

        // Create handler wrapper that calls the Python function
        // We need to clone the handler Py<PyAny> to move it into the Arc
        let handler_clone = handler.clone_ref(py);

        let wrapped_handler = std::sync::Arc::new(move |args: Vec<serde_json::Value>| {
            // Acquire GIL to call Python function
            Python::attach(|py| {
                // Convert JSON args to Python objects
                let py_args = args
                    .iter()
                    .map(|arg| json_to_python(py, arg))
                    .collect::<PyResult<Vec<_>>>()
                    .map_err(|e| format!("Failed to convert arguments: {}", e))?;

                // Call the Python function
                let result = handler_clone
                    .call1(py, (py_args,))
                    .map_err(|e| format!("Python handler error: {}", e))?;

                // Convert result back to JSON
                python_to_json(result.into_bound(py))
                    .map_err(|e| format!("Failed to convert result: {}", e))
            })
        });

        // Use RuntimeHandle's register_op method
        let op_id = handle
            .register_op(name, op_mode, op_permissions, wrapped_handler)
            .map_err(|e| PyRuntimeError::new_err(format!("Op registration failed: {}", e)))?;

        Ok(op_id)
    }

    /// Create a new context in this runtime.
    ///
    /// Returns:
    ///     RuntimeContext: A new context handle
    ///
    /// Raises:
    ///     RuntimeError: If context creation fails or runtime is closed
    fn create_context(&self) -> PyResult<super::context::RuntimeContext> {
        let handle_opt = self.handle.borrow();
        let handle = handle_opt
            .as_ref()
            .ok_or_else(|| PyRuntimeError::new_err("Runtime has been closed"))?;

        let context_id = handle
            .create_context()
            .map_err(|e| PyRuntimeError::new_err(format!("Failed to create context: {}", e)))?;

        Ok(super::context::RuntimeContext::new(
            context_id,
            handle.clone(),
        ))
    }
}

/// Parse a permission string into a Permission enum.
fn parse_permission(perm_str: &str) -> PyResult<super::ops::Permission> {
    if perm_str == "timers" {
        Ok(super::ops::Permission::Timers)
    } else if perm_str == "env" {
        Ok(super::ops::Permission::Env)
    } else if perm_str == "process" {
        Ok(super::ops::Permission::Process)
    } else if let Some(host) = perm_str.strip_prefix("net:") {
        if host.is_empty() {
            Ok(super::ops::Permission::Net(None))
        } else {
            Ok(super::ops::Permission::Net(Some(host.to_string())))
        }
    } else if let Some(path) = perm_str.strip_prefix("file:") {
        if path.is_empty() {
            Ok(super::ops::Permission::File(None))
        } else {
            Ok(super::ops::Permission::File(Some(path.to_string())))
        }
    } else {
        Ok(super::ops::Permission::Custom(perm_str.to_string()))
    }
}

/// Convert JSON value to Python object.
fn json_to_python(py: Python<'_>, value: &serde_json::Value) -> PyResult<Py<PyAny>> {
    use pyo3::types::{PyBool, PyFloat, PyInt, PyString};

    match value {
        serde_json::Value::Null => Ok(py.None()),
        serde_json::Value::Bool(b) => {
            let py_bool = PyBool::new(py, *b).to_owned();
            Ok(py_bool.into())
        }
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                let py_int = PyInt::new(py, i).to_owned();
                Ok(py_int.into())
            } else if let Some(f) = n.as_f64() {
                let py_float = PyFloat::new(py, f).to_owned();
                Ok(py_float.into())
            } else {
                Err(PyRuntimeError::new_err("Unsupported number type"))
            }
        }
        serde_json::Value::String(s) => {
            let py_str = PyString::new(py, s).to_owned();
            Ok(py_str.into())
        }
        serde_json::Value::Array(items) => {
            let py_list = pyo3::types::PyList::empty(py);
            for item in items {
                py_list.append(json_to_python(py, item)?)?;
            }
            Ok(py_list.into())
        }
        serde_json::Value::Object(map) => {
            let py_dict = pyo3::types::PyDict::new(py);
            for (key, value) in map {
                py_dict.set_item(key, json_to_python(py, value)?)?;
            }
            Ok(py_dict.into())
        }
    }
}

/// Convert Python object to JSON value.
fn python_to_json(obj: Bound<'_, PyAny>) -> PyResult<serde_json::Value> {
    if obj.is_none() {
        Ok(serde_json::Value::Null)
    } else if let Ok(b) = obj.extract::<bool>() {
        Ok(serde_json::Value::Bool(b))
    } else if let Ok(i) = obj.extract::<i64>() {
        Ok(serde_json::json!(i))
    } else if let Ok(f) = obj.extract::<f64>() {
        Ok(serde_json::json!(f))
    } else if let Ok(s) = obj.extract::<String>() {
        Ok(serde_json::Value::String(s))
    } else if let Ok(list) = obj.cast::<pyo3::types::PyList>() {
        let mut items = Vec::new();
        for item in list.iter() {
            items.push(python_to_json(item)?);
        }
        Ok(serde_json::Value::Array(items))
    } else if let Ok(dict) = obj.cast::<pyo3::types::PyDict>() {
        let mut map = serde_json::Map::new();
        for (key, value) in dict.iter() {
            let key_str = key.extract::<String>()?;
            map.insert(key_str, python_to_json(value)?);
        }
        Ok(serde_json::Value::Object(map))
    } else {
        Err(PyRuntimeError::new_err(
            "Unsupported Python type for JSON conversion",
        ))
    }
}

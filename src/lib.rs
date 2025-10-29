use pyo3::prelude::*;

mod runtime;

// Exception raised when a JavaScript function throws an error
pyo3::create_exception!(v8, JavaScriptError, pyo3::exceptions::PyException);

// Exception raised when a promise times out during await
pyo3::create_exception!(v8, PromiseTimeoutError, pyo3::exceptions::PyException);

// Base exception for V8 errors
pyo3::create_exception!(v8, V8Error, pyo3::exceptions::PyException);

// Module-specific exceptions
pyo3::create_exception!(v8, ModuleError, V8Error);
pyo3::create_exception!(v8, ModuleCompileError, ModuleError);
pyo3::create_exception!(v8, ModuleInstantiationError, ModuleError);
pyo3::create_exception!(v8, ModuleEvaluationError, ModuleError);

// V8Runtime has been removed. See LEGACY.md for the archived implementation.
// Use Isolate + Context instead.

/// Python v8 module
///
/// This module provides Python bindings to the V8 JavaScript engine.
///
/// Main classes:
/// - Runtime: Tokio-based async JavaScript runtime
/// - RuntimeContext: Execution context for JavaScript code
///
/// Exceptions:
/// - JavaScriptError: Raised when a JavaScript function throws an error
/// - PromiseTimeoutError: Raised when a promise times out during await
/// - V8Error: Base exception for V8 errors
#[pymodule]
fn _v8(m: &Bound<'_, PyModule>) -> PyResult<()> {
    // Tokio-based runtime
    m.add_class::<runtime::python::Runtime>()?;
    m.add_class::<runtime::context::RuntimeContext>()?;

    m.add("JavaScriptError", m.py().get_type::<JavaScriptError>())?;
    m.add(
        "PromiseTimeoutError",
        m.py().get_type::<PromiseTimeoutError>(),
    )?;
    m.add("V8Error", m.py().get_type::<V8Error>())?;
    Ok(())
}

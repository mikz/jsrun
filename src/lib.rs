use pyo3::prelude::*;

mod runtime;

// Exception raised when a JavaScript function throws an error
pyo3::create_exception!(v8, JavaScriptError, pyo3::exceptions::PyException);

// Exception raised when a promise times out during await
pyo3::create_exception!(v8, PromiseTimeoutError, pyo3::exceptions::PyException);

// Base exception for V8 errors
pyo3::create_exception!(v8, V8Error, pyo3::exceptions::PyException);

// Module-specific exceptions
/// Python jsrun module
///
/// This module provides Python bindings to the jsrun JavaScript runtime.
#[pymodule]
fn _jsrun(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<runtime::python::Runtime>()?;

    m.add("JavaScriptError", m.py().get_type::<JavaScriptError>())?;
    m.add(
        "PromiseTimeoutError",
        m.py().get_type::<PromiseTimeoutError>(),
    )?;
    m.add("V8Error", m.py().get_type::<V8Error>())?;
    Ok(())
}

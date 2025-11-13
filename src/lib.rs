use pyo3::prelude::*;

mod runtime;

/// Python jsrun module
///
/// This module provides Python bindings to the jsrun JavaScript runtime.
#[pymodule]
fn _jsrun(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<runtime::python::Runtime>()?;
    m.add_class::<runtime::python::JsFunction>()?;
    m.add_class::<runtime::python::JsStream>()?;
    m.add_class::<runtime::python::JsUndefined>()?;
    m.add_class::<runtime::python::RuntimeStats>()?;
    m.add_class::<runtime::python::InspectorEndpoints>()?;
    m.add_class::<runtime::python::JsFunctionFinalizer>()?;
    m.add_class::<runtime::python::JsStreamFinalizer>()?;
    m.add_class::<runtime::python::PyStreamSource>()?;
    m.add_class::<runtime::python::PyStreamFinalizer>()?;
    m.add_class::<runtime::python::SnapshotBuilderPy>()?;
    m.add_class::<runtime::RuntimeConfig>()?;
    m.add_class::<runtime::config::InspectorConfig>()?;
    let js_error_type = m.py().get_type::<runtime::python::JavaScriptError>();
    js_error_type.setattr("__module__", "jsrun")?;
    m.add("JavaScriptError", js_error_type)?;

    let runtime_terminated_type = m.py().get_type::<runtime::python::RuntimeTerminated>();
    runtime_terminated_type.setattr("__module__", "jsrun")?;
    m.add("RuntimeTerminated", runtime_terminated_type)?;
    let undefined: Py<PyAny> = runtime::python::get_js_undefined(m.py())?.into();
    m.add("undefined", undefined)?;
    m.add_function(pyo3::wrap_pyfunction!(
        runtime::python::_debug_active_runtime_threads,
        m
    )?)?;
    Ok(())
}

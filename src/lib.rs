use pyo3::prelude::*;

mod runtime;

/// Python jsrun module
///
/// This module provides Python bindings to the jsrun JavaScript runtime.
#[pymodule]
fn _jsrun(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<runtime::python::Runtime>()?;
    m.add_class::<runtime::RuntimeConfig>()?;
    Ok(())
}

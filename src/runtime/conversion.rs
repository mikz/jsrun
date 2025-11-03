//! Conversion helpers between Python objects and JSValue/serde_json values.

use crate::runtime::js_value::{JSValue, LimitTracker, MAX_JS_BYTES, MAX_JS_DEPTH};
use indexmap::IndexMap;
use pyo3::exceptions::PyRuntimeError;
use pyo3::prelude::*;
use pyo3::types::{PyBool, PyDict, PyFloat, PyInt, PyList, PyString};
use std::collections::HashSet;

/// Convert a JSValue into a Python object.
///
/// This is the new primary conversion function that supports native JavaScript values
/// including NaN and ±Infinity without sentinel strings.
///
/// For Function variants, a RuntimeHandle must be provided to create JsFunction proxies.
pub(crate) fn js_value_to_python(
    py: Python<'_>,
    value: &JSValue,
    handle: Option<&super::handle::RuntimeHandle>,
) -> PyResult<Py<PyAny>> {
    match value {
        JSValue::Null => Ok(py.None()),
        JSValue::Bool(b) => Ok(PyBool::new(py, *b).to_owned().into_any().unbind()),
        JSValue::Int(i) => Ok(PyInt::new(py, *i).into()),
        JSValue::Float(f) => Ok(PyFloat::new(py, *f).into()),
        JSValue::String(s) => Ok(PyString::new(py, s).into()),
        JSValue::Array(items) => {
            let list = PyList::empty(py);
            for item in items {
                list.append(js_value_to_python(py, item, handle)?)?;
            }
            Ok(list.into())
        }
        JSValue::Object(map) => {
            let dict = PyDict::new(py);
            for (key, val) in map {
                dict.set_item(key, js_value_to_python(py, val, handle)?)?;
            }
            Ok(dict.into())
        }
        JSValue::Function { id } => {
            // Create JsFunction proxy
            let handle = handle.ok_or_else(|| {
                PyRuntimeError::new_err("RuntimeHandle required to convert JSValue::Function")
            })?;

            let js_fn = super::python::JsFunction::new(handle.clone(), *id)?;
            let py_obj: Py<PyAny> = Py::new(py, js_fn)?.into();
            Ok(py_obj)
        }
    }
}

/// Convert a Python object into a JSValue.
///
/// This is used by the ops system to convert Python handler arguments to JSValue.
pub(crate) fn python_to_js_value(obj: Bound<'_, PyAny>) -> PyResult<JSValue> {
    let mut seen: HashSet<usize> = HashSet::new();
    let mut tracker = LimitTracker::new(MAX_JS_DEPTH, MAX_JS_BYTES);
    python_to_js_value_internal(obj, 0, &mut seen, &mut tracker)
}

fn python_to_js_value_internal(
    obj: Bound<'_, PyAny>,
    depth: usize,
    seen: &mut HashSet<usize>,
    tracker: &mut LimitTracker,
) -> PyResult<JSValue> {
    if depth > MAX_JS_DEPTH {
        return Err(PyRuntimeError::new_err(format!(
            "Depth limit exceeded: {} > {}",
            depth, MAX_JS_DEPTH
        )));
    }

    tracker.enter().map_err(PyRuntimeError::new_err)?;

    let add_bytes = |bytes: usize, tracker: &mut LimitTracker| {
        tracker.add_bytes(bytes).map_err(PyRuntimeError::new_err)
    };

    let result = if obj.is_none() {
        add_bytes(4, tracker)?;
        Ok(JSValue::Null)
    } else if let Ok(list) = obj.cast::<PyList>() {
        let ptr = list.as_ptr() as usize;
        if !seen.insert(ptr) {
            return Err(PyRuntimeError::new_err(
                "Circular reference detected while converting Python list",
            ));
        }

        add_bytes(16, tracker)?;
        add_bytes(
            list.len().saturating_mul(std::mem::size_of::<usize>()),
            tracker,
        )?;

        let mut items = Vec::with_capacity(list.len());
        for item in list.iter() {
            items.push(python_to_js_value_internal(item, depth + 1, seen, tracker)?);
        }
        seen.remove(&ptr);
        Ok(JSValue::Array(items))
    } else if let Ok(dict) = obj.cast::<PyDict>() {
        let ptr = dict.as_ptr() as usize;
        if !seen.insert(ptr) {
            return Err(PyRuntimeError::new_err(
                "Circular reference detected while converting Python dict",
            ));
        }

        add_bytes(24, tracker)?;
        add_bytes(
            dict.len().saturating_mul(std::mem::size_of::<usize>() * 2),
            tracker,
        )?;

        let mut map = IndexMap::with_capacity(dict.len());
        for (key, value) in dict.iter() {
            let key_str = key.extract::<String>()?;
            add_bytes(key_str.len(), tracker)?;
            add_bytes(8, tracker)?;
            map.insert(
                key_str,
                python_to_js_value_internal(value, depth + 1, seen, tracker)?,
            );
        }
        seen.remove(&ptr);
        Ok(JSValue::Object(map))
    } else if let Ok(b) = obj.extract::<bool>() {
        add_bytes(1, tracker)?;
        Ok(JSValue::Bool(b))
    } else if let Ok(i) = obj.extract::<i64>() {
        add_bytes(std::mem::size_of::<i64>(), tracker)?;
        Ok(JSValue::Int(i))
    } else if let Ok(f) = obj.extract::<f64>() {
        add_bytes(std::mem::size_of::<f64>(), tracker)?;
        Ok(JSValue::Float(f))
    } else if let Ok(s) = obj.extract::<String>() {
        if s.len() > MAX_JS_BYTES {
            return Err(PyRuntimeError::new_err(format!(
                "String size limit exceeded: {} > {}",
                s.len(),
                MAX_JS_BYTES
            )));
        }
        add_bytes(s.len(), tracker)?;
        add_bytes(16, tracker)?;
        Ok(JSValue::String(s))
    } else if let Ok(js_fn) = obj.extract::<pyo3::PyRef<super::python::JsFunction>>() {
        // JsFunction proxy - extract the function ID for round-trip
        // This validates that the function is not closed and runtime is alive
        let id = js_fn.function_id_for_transfer()?;
        add_bytes(8, tracker)?;
        Ok(JSValue::Function { id })
    } else {
        Err(PyRuntimeError::new_err(
            "Unsupported Python type for JSValue conversion",
        ))
    };

    tracker.exit();
    result
}

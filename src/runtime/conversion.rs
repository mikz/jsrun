//! Conversion helpers between Python objects and serde_json values.

use pyo3::exceptions::PyRuntimeError;
use pyo3::prelude::*;
use pyo3::types::{PyBool, PyDict, PyFloat, PyInt, PyList, PyString};

/// Convert a JSON value into a Python object.
pub(crate) fn json_to_python(py: Python<'_>, value: &serde_json::Value) -> PyResult<Py<PyAny>> {
    match value {
        serde_json::Value::Null => Ok(py.None()),
        serde_json::Value::Bool(b) => Ok(PyBool::new(py, *b).to_owned().into_any().unbind()),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Ok(PyInt::new(py, i).into())
            } else if let Some(f) = n.as_f64() {
                Ok(PyFloat::new(py, f).into())
            } else {
                Err(PyRuntimeError::new_err("Unsupported number type"))
            }
        }
        serde_json::Value::String(s) => Ok(PyString::new(py, s).into()),
        serde_json::Value::Array(items) => {
            let list = PyList::empty(py);
            for item in items {
                list.append(json_to_python(py, item)?)?;
            }
            Ok(list.into())
        }
        serde_json::Value::Object(map) => {
            let dict = PyDict::new(py);
            for (key, val) in map {
                dict.set_item(key, json_to_python(py, val)?)?;
            }
            Ok(dict.into())
        }
    }
}

/// Convert a Python object into a JSON value.
pub(crate) fn python_to_json(obj: Bound<'_, PyAny>) -> PyResult<serde_json::Value> {
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
    } else if let Ok(list) = obj.cast::<PyList>() {
        let mut items = Vec::with_capacity(list.len());
        for item in list.iter() {
            items.push(python_to_json(item)?);
        }
        Ok(serde_json::Value::Array(items))
    } else if let Ok(dict) = obj.cast::<PyDict>() {
        let mut map = serde_json::Map::with_capacity(dict.len());
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

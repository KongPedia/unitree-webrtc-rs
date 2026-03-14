use pyo3::exceptions::{PyNotImplementedError, PyRuntimeError};
use pyo3::prelude::*;
use pyo3::types::{PyAny, PyBool, PyDict, PyList};
use serde_json::{Map, Number, Value};

pub fn py_any_to_json_value(object: &Bound<'_, PyAny>) -> PyResult<Value> {
    if object.is_none() {
        return Ok(Value::Null);
    }

    if let Ok(v) = object.extract::<bool>() {
        return Ok(Value::Bool(v));
    }

    if let Ok(v) = object.extract::<i64>() {
        return Ok(Value::Number(Number::from(v)));
    }

    if let Ok(v) = object.extract::<f64>() {
        return Ok(Number::from_f64(v)
            .map(Value::Number)
            .unwrap_or(Value::Null));
    }

    if let Ok(v) = object.extract::<String>() {
        return Ok(Value::String(v));
    }

    if let Ok(list) = object.clone().cast_exact::<PyList>() {
        let arr: Result<Vec<_>, _> = list
            .iter()
            .map(|item| py_any_to_json_value(&item))
            .collect();
        return Ok(Value::Array(arr?));
    }

    if let Ok(dict) = object.clone().cast_exact::<PyDict>() {
        let mut map = Map::new();
        for (key, value) in dict.iter() {
            let key_str = key.extract::<String>()?;
            let json_value = py_any_to_json_value(&value)?;
            map.insert(key_str, json_value);
        }
        return Ok(Value::Object(map));
    }

    Err(PyErr::new::<pyo3::exceptions::PyTypeError, _>(
        "Unsupported type for JSON conversion",
    ))
}

pub fn json_value_to_py(py: Python<'_>, value: &Value) -> PyResult<Py<PyAny>> {
    match value {
        Value::Null => Ok(py.None()),
        Value::Bool(v) => Ok(PyBool::new(py, *v).to_owned().into_any().unbind()),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Ok(i.into_pyobject(py)?.into_any().unbind())
            } else if let Some(f) = n.as_f64() {
                Ok(f.into_pyobject(py)?.into_any().unbind())
            } else {
                Ok(py.None())
            }
        }
        Value::String(s) => Ok(s.into_pyobject(py)?.into_any().unbind()),
        Value::Array(arr) => {
            let list = PyList::empty(py);
            for item in arr {
                let py_item = json_value_to_py(py, item)?;
                list.append(py_item)?;
            }
            Ok(list.into_any().unbind())
        }
        Value::Object(obj) => {
            let dict = PyDict::new(py);
            for (key, value) in obj {
                let py_value = json_value_to_py(py, value)?;
                dict.set_item(key, py_value)?;
            }
            Ok(dict.into_any().unbind())
        }
    }
}

pub fn to_py_error(error: String) -> PyErr {
    if error.contains("not implemented") {
        return PyNotImplementedError::new_err(error);
    }
    PyRuntimeError::new_err(error)
}

use pyo3::exceptions::{PyNotImplementedError, PyRuntimeError, PyTypeError, PyValueError};
use pyo3::prelude::*;
use pyo3::types::{PyAny, PyBool, PyDict, PyList};
use serde_json::{Map, Number, Value};
use std::collections::HashSet;

pub(crate) async fn with_session_owner<T>(
    session: std::sync::Arc<crate::connection::UnitreeSession>,
    operation: impl std::future::Future<Output = T>,
) -> T {
    let result = operation.await;
    drop(session);
    result
}

// Bound container nesting, not scalar leaves, before recursive conversion.
const MAX_JSON_DEPTH: usize = 64;

pub fn py_any_to_json_value(object: &Bound<'_, PyAny>) -> PyResult<Value> {
    convert_json(object, 0, &mut HashSet::new())
}

fn convert_json(
    object: &Bound<'_, PyAny>,
    depth: usize,
    active_containers: &mut HashSet<usize>,
) -> PyResult<Value> {
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
        return Number::from_f64(v)
            .map(Value::Number)
            .ok_or_else(|| PyValueError::new_err("JSON numbers must be finite"));
    }
    if let Ok(v) = object.extract::<String>() {
        return Ok(Value::String(v));
    }

    let list = object.cast_exact::<PyList>().ok();
    let dict = object.cast_exact::<PyDict>().ok();
    if list.is_none() && dict.is_none() {
        return Err(PyTypeError::new_err("Unsupported type for JSON conversion"));
    }
    if depth >= MAX_JSON_DEPTH {
        return Err(PyValueError::new_err("JSON container depth exceeds 64"));
    }
    let identity = object.as_ptr() as usize;
    if !active_containers.insert(identity) {
        return Err(PyValueError::new_err("Cyclic JSON container"));
    }

    // Track only the current path: repeated shared containers are valid JSON.
    let result = match (list, dict) {
        (Some(list), _) => list
            .iter()
            .map(|item| convert_json(&item, depth + 1, active_containers))
            .collect::<PyResult<Vec<_>>>()
            .map(Value::Array),
        (_, Some(dict)) => dict
            .iter()
            .map(|(key, value)| {
                Ok((
                    key.extract::<String>()?,
                    convert_json(&value, depth + 1, active_containers)?,
                ))
            })
            .collect::<PyResult<Map<_, _>>>()
            .map(Value::Object),
        _ => unreachable!("container type checked above"),
    };
    active_containers.remove(&identity);
    result
}

pub fn ensure_callable(callback: &Bound<'_, PyAny>) -> PyResult<()> {
    if callback.is_callable() {
        return Ok(());
    }
    Err(PyTypeError::new_err("callback must be callable"))
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

#[cfg(test)]
mod tests {
    use super::*;
    use pyo3::exceptions::PyValueError;

    #[test]
    fn rejects_nonfinite_numbers_and_excessive_depth() {
        Python::initialize();
        Python::attach(|py| {
            for value in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
                let object = value.into_pyobject(py).unwrap();
                assert!(py_any_to_json_value(object.as_any())
                    .unwrap_err()
                    .is_instance_of::<PyValueError>(py));
            }
            let mut nested = py.None();
            for _ in 0..65 {
                nested = PyList::new(py, [nested]).unwrap().into_any().unbind();
            }
            assert!(py_any_to_json_value(nested.bind(py))
                .unwrap_err()
                .is_instance_of::<PyValueError>(py));
        });
    }
    #[test]
    fn cycles_fail_but_shared_containers_are_valid() {
        Python::initialize();
        Python::attach(|py| {
            let list = PyList::empty(py);
            list.append(&list).unwrap();
            assert!(py_any_to_json_value(list.as_any())
                .unwrap_err()
                .is_instance_of::<PyValueError>(py));
            let dict = PyDict::new(py);
            dict.set_item("self", &dict).unwrap();
            assert!(py_any_to_json_value(dict.as_any())
                .unwrap_err()
                .is_instance_of::<PyValueError>(py));
            let shared = PyList::new(py, [1, 2]).unwrap();
            let dag = PyList::new(py, [&shared, &shared]).unwrap();
            assert_eq!(
                py_any_to_json_value(dag.as_any()).unwrap(),
                serde_json::json!([[1, 2], [1, 2]])
            );
        });
    }

    #[test]
    fn callback_validation_rejects_noncallable() {
        Python::initialize();
        Python::attach(|py| {
            assert!(ensure_callable(py.None().bind(py))
                .unwrap_err()
                .is_instance_of::<PyTypeError>(py));
        });
    }
    fn offline_session_owner() -> std::sync::Arc<crate::connection::UnitreeSession> {
        use crate::interface::bridges::connection::PyUnitreeWebRTCConnection;
        use crate::interface::bridges::datachannel::PyDataChannelBridge;
        use crate::protocol::constants::WebRTCConnectionMethod;
        Python::initialize();
        Python::attach(|py| {
            let connection = py
                .get_type::<PyUnitreeWebRTCConnection>()
                .call1((WebRTCConnectionMethod::LocalSTA as i32,))
                .unwrap();
            let channel = connection.getattr("datachannel").unwrap();
            let bridge: PyRef<'_, PyDataChannelBridge> = channel.extract().unwrap();
            std::sync::Arc::clone(&bridge._session)
        })
    }

    #[tokio::test]
    async fn async_operation_keeps_native_session_until_completion() {
        let session = offline_session_owner();
        let weak = std::sync::Arc::downgrade(&session);
        let (complete, pending) = tokio::sync::oneshot::channel::<()>();
        let (started, entered) = tokio::sync::oneshot::channel::<()>();
        let operation = tokio::spawn(with_session_owner(session, async move {
            started.send(()).unwrap();
            pending.await.unwrap()
        }));
        entered.await.unwrap();
        assert!(weak.upgrade().is_some());
        complete.send(()).unwrap();
        operation.await.unwrap();
        assert!(weak.upgrade().is_none());
    }

    #[tokio::test]
    async fn cancelling_async_operation_releases_native_session() {
        let session = offline_session_owner();
        let weak = std::sync::Arc::downgrade(&session);
        let (started, entered) = tokio::sync::oneshot::channel::<()>();
        let operation = tokio::spawn(with_session_owner(session, async move {
            started.send(()).unwrap();
            std::future::pending::<()>().await;
        }));
        entered.await.unwrap();
        assert!(weak.upgrade().is_some());
        operation.abort();
        assert!(operation.await.unwrap_err().is_cancelled());
        assert!(weak.upgrade().is_none());
    }
    #[tokio::test]
    async fn dropping_unpolled_async_operation_releases_native_session() {
        let session = offline_session_owner();
        let weak = std::sync::Arc::downgrade(&session);
        let operation = with_session_owner(session, std::future::pending::<()>());
        assert!(weak.upgrade().is_some());
        drop(operation);
        assert!(weak.upgrade().is_none());
    }
}

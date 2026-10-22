use crate::connection::UnitreeSession;
use crate::datachannel::core::validated_timeout;
use crate::datachannel::DataChannelService;
use crate::infrastructure::rtc_engine::RtcEngine;
use crate::interface::utils::{
    ensure_callable, json_value_to_py, py_any_to_json_value, to_py_error, with_session_owner,
};
use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

pub struct RegisteredCallback {
    pub id: u64,
    pub callback: Py<PyAny>,
}

impl From<Py<PyAny>> for RegisteredCallback {
    fn from(callback: Py<PyAny>) -> Self {
        static NEXT_ID: AtomicU64 = AtomicU64::new(1);
        Self {
            id: NEXT_ID.fetch_add(1, Ordering::Relaxed),
            callback,
        }
    }
}

pub type CallbackRegistry = Arc<Mutex<HashMap<String, RegisteredCallback>>>;

#[pyclass(name = "PubSubBridge", unsendable)]
pub struct PyPubSubBridge {
    pub service: Arc<DataChannelService<RtcEngine>>,
    pub callback_registry: CallbackRegistry,
    pub _session: Arc<UnitreeSession>,
}

#[pymethods]
impl PyPubSubBridge {
    #[pyo3(signature = (topic, options, timeout=10.0))]
    fn publish_request_new<'py>(
        &self,
        py: Python<'py>,
        topic: &str,
        options: Py<PyAny>,
        timeout: f64,
    ) -> PyResult<Bound<'py, PyAny>> {
        validated_timeout(timeout).map_err(PyValueError::new_err)?;
        let options_value = py_any_to_json_value(options.bind(py))?;
        let topic_string = topic.to_string();
        let service = Arc::clone(&self.service);
        let session = Arc::clone(&self._session);

        pyo3_async_runtimes::tokio::future_into_py(
            py,
            with_session_owner(session, async move {
                let result = service
                    .publish_request_new(&topic_string, options_value, Some(timeout))
                    .await
                    .map_err(to_py_error)?;

                Python::attach(|py| json_value_to_py(py, &result))
            }),
        )
    }

    #[pyo3(signature = (topic, data=None, msg_type=None, timeout=10.0))]
    fn publish<'py>(
        &self,
        py: Python<'py>,
        topic: &str,
        data: Option<Py<PyAny>>,
        msg_type: Option<&str>,
        timeout: f64,
    ) -> PyResult<Bound<'py, PyAny>> {
        validated_timeout(timeout).map_err(PyValueError::new_err)?;
        let data_value = match data {
            Some(data) => Some(py_any_to_json_value(data.bind(py))?),
            None => None,
        };
        let topic_string = topic.to_string();
        let msg_type_string = msg_type.map(ToString::to_string);
        let service = Arc::clone(&self.service);
        let session = Arc::clone(&self._session);

        pyo3_async_runtimes::tokio::future_into_py(
            py,
            with_session_owner(session, async move {
                let result = service
                    .publish(
                        &topic_string,
                        data_value,
                        msg_type_string.as_deref(),
                        Some(timeout),
                    )
                    .await
                    .map_err(to_py_error)?;

                Python::attach(|py| json_value_to_py(py, &result))
            }),
        )
    }

    #[pyo3(signature = (topic, data=None, msg_type=None))]
    fn publish_without_callback(
        &self,
        py: Python<'_>,
        topic: &str,
        data: Option<Py<PyAny>>,
        msg_type: Option<&str>,
    ) -> PyResult<()> {
        let data_value = match data {
            Some(data) => Some(py_any_to_json_value(data.bind(py))?),
            None => None,
        };

        self.service
            .publish_without_callback(topic, data_value, msg_type)
            .map_err(to_py_error)
    }

    fn subscribe(&self, py: Python<'_>, topic: &str, callback: Py<PyAny>) -> PyResult<()> {
        ensure_callable(callback.bind(py))?;
        let previous = {
            let mut registry = self.callback_registry.lock().unwrap();
            self.service.subscribe(topic).map_err(to_py_error)?;
            registry.insert(topic.to_string(), callback.into())
        };
        // Python finalizers can execute user code; release their references outside the lock.
        drop(previous);
        Ok(())
    }

    fn unsubscribe(&self, topic: &str) -> PyResult<()> {
        let previous = {
            let mut registry = self.callback_registry.lock().unwrap();
            self.service.unsubscribe(topic).map_err(to_py_error)?;
            registry.remove(topic)
        };
        drop(previous);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::interface::bridges::connection::PyUnitreeWebRTCConnection;
    use crate::protocol::constants::WebRTCConnectionMethod;

    #[test]
    fn rejected_subscription_changes_preserve_exact_callback_registration() {
        Python::initialize();
        Python::attach(|py| {
            let connection = py
                .get_type::<PyUnitreeWebRTCConnection>()
                .call1((WebRTCConnectionMethod::LocalSTA as i32,))
                .unwrap();
            let pubsub = connection
                .getattr("datachannel")
                .unwrap()
                .getattr("pub_sub")
                .unwrap();
            let bridge: PyRef<'_, PyPubSubBridge> = pubsub.extract().unwrap();
            let previous = py
                .eval(c"lambda _message: None", None, None)
                .unwrap()
                .unbind();
            let replacement = py
                .eval(c"lambda _message: None", None, None)
                .unwrap()
                .unbind();

            assert!(bridge
                .subscribe(py, "topic", replacement.clone_ref(py))
                .is_err());
            assert!(bridge.callback_registry.lock().unwrap().is_empty());

            bridge
                .callback_registry
                .lock()
                .unwrap()
                .insert("topic".into(), previous.clone_ref(py).into());
            assert!(bridge.subscribe(py, "topic", replacement).is_err());
            assert_eq!(
                bridge.callback_registry.lock().unwrap()["topic"]
                    .callback
                    .as_ptr(),
                previous.as_ptr()
            );

            assert!(bridge.unsubscribe("topic").is_err());
            assert_eq!(
                bridge.callback_registry.lock().unwrap()["topic"]
                    .callback
                    .as_ptr(),
                previous.as_ptr()
            );
        });
    }
}

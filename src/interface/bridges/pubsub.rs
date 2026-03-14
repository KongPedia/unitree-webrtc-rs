use crate::datachannel::DataChannelService;
use crate::infrastructure::rtc_engine::RtcEngine;
use crate::interface::utils::{json_value_to_py, py_any_to_json_value, to_py_error};
use pyo3::prelude::*;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

pub type CallbackRegistry = Arc<Mutex<HashMap<String, Py<PyAny>>>>;

#[pyclass(name = "PubSubBridge", unsendable)]
pub struct PyPubSubBridge {
    pub service: Arc<DataChannelService<RtcEngine>>,
    pub callback_registry: CallbackRegistry,
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
        let options_value = py_any_to_json_value(options.bind(py))?;
        let topic_string = topic.to_string();
        let service = Arc::clone(&self.service);

        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let result = service
                .publish_request_new(&topic_string, options_value, Some(timeout))
                .await
                .map_err(to_py_error)?;

            Python::attach(|py| json_value_to_py(py, &result))
        })
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
        let data_value = match data {
            Some(data) => Some(py_any_to_json_value(data.bind(py))?),
            None => None,
        };
        let topic_string = topic.to_string();
        let msg_type_string = msg_type.map(ToString::to_string);
        let service = Arc::clone(&self.service);

        pyo3_async_runtimes::tokio::future_into_py(py, async move {
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
        })
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

    fn subscribe(&self, topic: &str, callback: Py<PyAny>) -> PyResult<()> {
        {
            let mut registry = self.callback_registry.lock().unwrap();
            registry.insert(topic.to_string(), callback);
        }
        self.service.subscribe(topic).map_err(to_py_error)
    }

    fn unsubscribe(&self, topic: &str) -> PyResult<()> {
        {
            let mut registry = self.callback_registry.lock().unwrap();
            registry.remove(topic);
        }
        self.service.unsubscribe(topic).map_err(to_py_error)
    }
}

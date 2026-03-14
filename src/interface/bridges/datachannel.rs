use crate::datachannel::DataChannelService;
use crate::infrastructure::rtc_engine::RtcEngine;
use crate::interface::bridges::pubsub::{CallbackRegistry, PyPubSubBridge};
use crate::interface::utils::to_py_error;
use pyo3::prelude::*;
use std::sync::Arc;

#[pyclass(name = "DataChannelBridge", unsendable)]
pub struct PyDataChannelBridge {
    pub service: Arc<DataChannelService<RtcEngine>>,
    pub callback_registry: CallbackRegistry,
}

#[pymethods]
impl PyDataChannelBridge {
    #[getter]
    fn pub_sub(&self, py: Python<'_>) -> PyResult<Py<PyPubSubBridge>> {
        Py::new(
            py,
            PyPubSubBridge {
                service: Arc::clone(&self.service),
                callback_registry: Arc::clone(&self.callback_registry),
            },
        )
    }

    #[pyo3(name = "disableTrafficSaving")]
    #[pyo3(signature = (switch=true))]
    fn disable_traffic_saving<'py>(
        &self,
        py: Python<'py>,
        switch: bool,
    ) -> PyResult<Bound<'py, PyAny>> {
        let service = Arc::clone(&self.service);
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            service
                .disable_traffic_saving(switch)
                .await
                .map_err(to_py_error)
        })
    }

    #[pyo3(name = "switchVideoChannel")]
    fn switch_video_channel(&self, switch: bool) -> PyResult<()> {
        self.service
            .switch_video_channel(switch)
            .map_err(to_py_error)
    }

    #[pyo3(name = "switchAudioChannel")]
    fn switch_audio_channel(&self, switch: bool) -> PyResult<()> {
        self.service
            .switch_audio_channel(switch)
            .map_err(to_py_error)
    }
}

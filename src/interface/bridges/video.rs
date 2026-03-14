use crate::datachannel::DataChannelService;
use crate::infrastructure::rtc_engine::RtcEngine;
use crate::interface::bridges::pubsub::CallbackRegistry;
use crate::interface::utils::to_py_error;
use pyo3::prelude::*;
use std::sync::Arc;

#[pyclass(name = "VideoBridge", unsendable)]
pub struct PyVideoBridge {
    pub datachannel_service: Arc<DataChannelService<RtcEngine>>,
    pub callback_registry: CallbackRegistry,
}

#[pymethods]
impl PyVideoBridge {
    #[pyo3(name = "switchVideoChannel")]
    fn switch_video_channel(&self, switch: bool) -> PyResult<()> {
        self.datachannel_service
            .switch_video_channel(switch)
            .map_err(to_py_error)
    }

    fn on_frame(&self, callback: Py<PyAny>) -> PyResult<()> {
        let mut registry = self.callback_registry.lock().unwrap();
        registry.insert("video".to_string(), callback);
        Ok(())
    }
}

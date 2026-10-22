use crate::connection::UnitreeSession;
use crate::datachannel::DataChannelService;
use crate::infrastructure::rtc_engine::RtcEngine;
use crate::interface::bridges::pubsub::CallbackRegistry;
use crate::interface::utils::{ensure_callable, to_py_error};
use pyo3::prelude::*;
use std::sync::Arc;

#[pyclass(name = "VideoBridge", unsendable)]
pub struct PyVideoBridge {
    pub datachannel_service: Arc<DataChannelService<RtcEngine>>,
    pub callback_registry: CallbackRegistry,
    pub _session: Arc<UnitreeSession>,
}

#[pymethods]
impl PyVideoBridge {
    #[pyo3(name = "switchVideoChannel")]
    fn switch_video_channel(&self, switch: bool) -> PyResult<()> {
        self.datachannel_service
            .switch_video_channel(switch)
            .map_err(to_py_error)
    }

    fn on_frame(&self, py: Python<'_>, callback: Py<PyAny>) -> PyResult<()> {
        ensure_callable(callback.bind(py))?;
        let previous = {
            let mut registry = self.callback_registry.lock().unwrap();
            registry.insert("video".to_string(), callback.into())
        };
        // Python finalizers may reenter registration when the last reference drops.
        drop(previous);
        Ok(())
    }
}

use crate::audio::transmit::AudioSource;
use crate::connection::UnitreeSession;
use crate::datachannel::DataChannelService;
use crate::infrastructure::rtc_engine::RtcEngine;
use crate::interface::bridges::pubsub::CallbackRegistry;
use crate::interface::utils::{ensure_callable, to_py_error};
use pyo3::prelude::*;
use std::sync::Arc;

#[pyclass(name = "AudioBridge", unsendable)]
pub struct PyAudioBridge {
    pub datachannel_service: Arc<DataChannelService<RtcEngine>>,
    pub callback_registry: CallbackRegistry,
    pub session: Arc<UnitreeSession>,
}

#[pymethods]
impl PyAudioBridge {
    #[pyo3(name = "switchAudioChannel")]
    fn switch_audio_channel(&self, switch: bool) -> PyResult<()> {
        self.datachannel_service
            .switch_audio_channel(switch)
            .map_err(to_py_error)
    }

    fn on_audio(&self, py: Python<'_>, callback: Py<PyAny>) -> PyResult<()> {
        ensure_callable(callback.bind(py))?;
        let previous = {
            let mut registry = self.callback_registry.lock().unwrap();
            registry.insert("audio".to_string(), callback.into())
        };
        // Finalizers may register another callback; never drop Python references under the lock.
        drop(previous);
        Ok(())
    }

    #[pyo3(name = "play_from_file")]
    fn play_from_file<'py>(&self, py: Python<'py>, path: String) -> PyResult<Bound<'py, PyAny>> {
        let session = Arc::clone(&self.session);
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            session
                .play_audio(AudioSource::File(path))
                .await
                .map_err(to_py_error)
        })
    }

    #[pyo3(name = "play_from_url")]
    fn play_from_url<'py>(&self, py: Python<'py>, url: String) -> PyResult<Bound<'py, PyAny>> {
        let session = Arc::clone(&self.session);
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            session
                .play_audio(AudioSource::Url(url))
                .await
                .map_err(to_py_error)
        })
    }

    fn stop<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let session = Arc::clone(&self.session);
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            session.stop_audio().await.map_err(to_py_error)
        })
    }
}

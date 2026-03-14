use crate::audio::AudioSender;
use crate::datachannel::DataChannelService;
use crate::infrastructure::rtc_engine::RtcEngine;
use crate::interface::bridges::pubsub::CallbackRegistry;
use crate::interface::utils::to_py_error;
use pyo3::prelude::*;
use std::sync::Arc;
use tokio::sync::Mutex as AsyncMutex;

#[pyclass(name = "AudioBridge", unsendable)]
pub struct PyAudioBridge {
    pub datachannel_service: Arc<DataChannelService<RtcEngine>>,
    pub callback_registry: CallbackRegistry,
    pub rtc_engine: Arc<RtcEngine>,
    pub audio_sender: Arc<AsyncMutex<Option<AudioSender>>>,
}

#[pymethods]
impl PyAudioBridge {
    #[pyo3(name = "switchAudioChannel")]
    fn switch_audio_channel(&self, switch: bool) -> PyResult<()> {
        self.datachannel_service
            .switch_audio_channel(switch)
            .map_err(to_py_error)
    }

    fn on_audio(&self, callback: Py<PyAny>) -> PyResult<()> {
        let mut registry = self.callback_registry.lock().unwrap();
        registry.insert("audio".to_string(), callback);
        Ok(())
    }

    #[pyo3(name = "play_from_file")]
    fn play_from_file<'py>(&self, py: Python<'py>, path: String) -> PyResult<Bound<'py, PyAny>> {
        let rtc_engine = Arc::clone(&self.rtc_engine);
        let audio_sender_arc = Arc::clone(&self.audio_sender);

        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            // Stop existing sender first to prevent multiple pipelines from mixing audio
            {
                let mut guard = audio_sender_arc.lock().await;
                if let Some(mut existing_sender) = guard.take() {
                    let _ = existing_sender.stop();
                }
            }

            let audio_track = rtc_engine.get_audio_track().await.ok_or_else(|| {
                PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(
                    "Audio track not initialized. Call connect() first.",
                )
            })?;

            let source = crate::audio::transmit::service::AudioSource::File(path);
            let sender = AudioSender::new(source, audio_track).map_err(to_py_error)?;

            sender.play().map_err(to_py_error)?;

            let mut guard = audio_sender_arc.lock().await;
            *guard = Some(sender);

            Ok(())
        })
    }

    #[pyo3(name = "play_from_url")]
    fn play_from_url<'py>(&self, py: Python<'py>, url: String) -> PyResult<Bound<'py, PyAny>> {
        let rtc_engine = Arc::clone(&self.rtc_engine);
        let audio_sender_arc = Arc::clone(&self.audio_sender);

        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            // Stop existing sender first to prevent multiple pipelines from mixing audio
            {
                let mut guard = audio_sender_arc.lock().await;
                if let Some(mut existing_sender) = guard.take() {
                    let _ = existing_sender.stop();
                }
            }

            let audio_track = rtc_engine.get_audio_track().await.ok_or_else(|| {
                PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(
                    "Audio track not initialized. Call connect() first.",
                )
            })?;

            let source = crate::audio::transmit::service::AudioSource::Url(url);
            let sender = AudioSender::new(source, audio_track).map_err(to_py_error)?;

            sender.play().map_err(to_py_error)?;

            let mut guard = audio_sender_arc.lock().await;
            *guard = Some(sender);

            Ok(())
        })
    }

    fn stop<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let audio_sender_arc = Arc::clone(&self.audio_sender);

        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let mut guard = audio_sender_arc.lock().await;
            if let Some(mut sender) = guard.take() {
                sender.stop().map_err(to_py_error)?;
            }
            Ok(())
        })
    }
}

use crate::audio::AudioSender;
use crate::connection::ConnectionService;
use crate::datachannel::lidar;
use crate::datachannel::DataChannelService;
use crate::infrastructure::rtc_engine::RtcEngine;
use crate::infrastructure::signaling_http::HttpSignalingClient;
use crate::interface::bridges::audio::PyAudioBridge;
use crate::interface::bridges::datachannel::PyDataChannelBridge;
use crate::interface::bridges::pubsub::CallbackRegistry;
use crate::interface::bridges::video::PyVideoBridge;
use crate::interface::callbacks::spawn_callback_dispatcher;
use crate::interface::utils::to_py_error;
use crate::protocol::constants::WebRTCConnectionMethod;
use crate::protocol::models::{CallbackEvent, DcMessage};
use crossbeam_channel::bounded;
use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;
use pyo3::types::PyDict;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tokio::sync::Mutex as AsyncMutex;

#[pyclass(name = "UnitreeWebRTCConnection")]
pub struct PyUnitreeWebRTCConnection {
    connection_method: WebRTCConnectionMethod,
    serial_number: Option<String>,
    ip: Option<String>,
    _username: Option<String>,
    _password: Option<String>,
    service: Arc<ConnectionService<HttpSignalingClient, RtcEngine>>,
    rtc_engine: Arc<RtcEngine>,
    datachannel_service: Arc<DataChannelService<RtcEngine>>,
    callback_registry: CallbackRegistry,
    audio_sender: Arc<AsyncMutex<Option<AudioSender>>>,
}

#[pymethods]
impl PyUnitreeWebRTCConnection {
    #[new]
    #[pyo3(signature = (connection_method, serial_number=None, ip=None, username=None, password=None, **kwargs))]
    fn new(
        connection_method: i32,
        serial_number: Option<String>,
        ip: Option<String>,
        username: Option<String>,
        password: Option<String>,
        kwargs: Option<&Bound<'_, PyDict>>,
    ) -> PyResult<Self> {
        let mut resolved_method = connection_method;
        let mut resolved_serial = serial_number.clone();

        if let Some(kwargs) = kwargs {
            if let Ok(Some(value)) = kwargs.get_item("connectionMethod") {
                resolved_method = value.extract::<i32>()?;
            }

            if let Ok(Some(value)) = kwargs.get_item("serialNumber") {
                resolved_serial = Some(value.extract::<String>()?);
            }
        }

        let connection_method =
            WebRTCConnectionMethod::try_from(resolved_method).map_err(PyValueError::new_err)?;

        // Queue sizes based on target environment:
        // - Local (Jetson): smaller queues due to memory constraints
        //   720p BGR frame ~2.6 MiB, so callback queue of 64 = ~166 MiB worst case
        // - Remote (Desktop/Server): larger queues with more memory available
        let (incoming_capacity, callback_capacity) = match connection_method {
            WebRTCConnectionMethod::LocalAP | WebRTCConnectionMethod::LocalSTA => (256, 64),
            WebRTCConnectionMethod::Remote => (512, 128),
        };

        let (incoming_tx, incoming_rx) = bounded::<DcMessage>(incoming_capacity);
        let (callback_events_tx, callback_events_rx) = bounded::<CallbackEvent>(callback_capacity);
        let callback_registry: CallbackRegistry = Arc::new(Mutex::new(HashMap::new()));
        spawn_callback_dispatcher(callback_events_rx, Arc::clone(&callback_registry));
        let lidar_worker_pool = lidar::create_worker_pool(callback_events_tx.clone());
        let signaling = Arc::new(HttpSignalingClient);
        let engine = Arc::new(RtcEngine::new(incoming_tx, callback_events_tx.clone()));
        let service = Arc::new(ConnectionService::new(
            signaling,
            Arc::clone(&engine),
            connection_method,
            ip.clone(),
        ));
        let datachannel_service = Arc::new(DataChannelService::new(
            Arc::clone(&engine),
            incoming_rx,
            callback_events_tx,
            lidar_worker_pool,
            connection_method == WebRTCConnectionMethod::Remote,
        ));

        Ok(Self {
            connection_method,
            serial_number: resolved_serial,
            ip,
            _username: username,
            _password: password,
            service,
            rtc_engine: engine,
            datachannel_service,
            callback_registry,
            audio_sender: Arc::new(AsyncMutex::new(None)),
        })
    }

    fn connect<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let service = Arc::clone(&self.service);
        let datachannel_service = Arc::clone(&self.datachannel_service);
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            service.connect().await.map_err(to_py_error)?;
            datachannel_service
                .wait_datachannel_open(5.0)
                .await
                .map_err(to_py_error)?;
            Ok(())
        })
    }

    fn disconnect<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let service = Arc::clone(&self.service);
        let datachannel_service = Arc::clone(&self.datachannel_service);
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            service.disconnect().await;
            datachannel_service.stop_background_tasks();
            Ok(())
        })
    }

    fn reconnect<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let service = Arc::clone(&self.service);
        let datachannel_service = Arc::clone(&self.datachannel_service);
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            service.reconnect().await.map_err(to_py_error)?;
            datachannel_service
                .wait_datachannel_open(5.0)
                .await
                .map_err(to_py_error)?;
            Ok(())
        })
    }

    #[pyo3(name = "_auto_reconnect")]
    #[pyo3(signature = (max_retries=5))]
    fn auto_reconnect_legacy<'py>(
        &self,
        py: Python<'py>,
        max_retries: u32,
    ) -> PyResult<Bound<'py, PyAny>> {
        let service = Arc::clone(&self.service);
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            Ok(service.auto_reconnect(max_retries).await)
        })
    }

    #[getter]
    fn connection_method(&self) -> i32 {
        self.connection_method as i32
    }

    #[getter(connectionMethod)]
    fn connection_method_legacy(&self) -> i32 {
        self.connection_method as i32
    }

    #[getter]
    fn is_connected(&self) -> bool {
        self.service.is_connected()
    }

    #[getter(isConnected)]
    fn is_connected_legacy(&self) -> bool {
        self.service.is_connected()
    }

    #[getter]
    fn ip(&self) -> Option<String> {
        self.ip.clone()
    }

    #[getter]
    fn video(&self, py: Python<'_>) -> PyResult<Py<PyVideoBridge>> {
        Py::new(
            py,
            PyVideoBridge {
                datachannel_service: Arc::clone(&self.datachannel_service),
                callback_registry: Arc::clone(&self.callback_registry),
            },
        )
    }

    #[getter]
    fn audio(&self, py: Python<'_>) -> PyResult<Py<PyAudioBridge>> {
        Py::new(
            py,
            PyAudioBridge {
                datachannel_service: Arc::clone(&self.datachannel_service),
                callback_registry: Arc::clone(&self.callback_registry),
                rtc_engine: Arc::clone(&self.rtc_engine),
                audio_sender: Arc::clone(&self.audio_sender),
            },
        )
    }

    #[getter]
    fn serial_number(&self) -> Option<String> {
        self.serial_number.clone()
    }

    #[getter]
    fn datachannel(&self, py: Python<'_>) -> PyResult<Py<PyDataChannelBridge>> {
        Py::new(
            py,
            PyDataChannelBridge {
                service: Arc::clone(&self.datachannel_service),
                callback_registry: Arc::clone(&self.callback_registry),
            },
        )
    }
}

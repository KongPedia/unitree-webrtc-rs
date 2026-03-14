use crate::application::connection_service::ConnectionService;
use crate::application::datachannel_service::DataChannelService;
use crate::application::lidar_service;
use crate::domain::models::{CallbackEvent, DcMessage};
use crate::infrastructure::rtc_engine::RtcEngine;
use crate::infrastructure::signaling_http::HttpSignalingClient;
use crate::interface::constants::WebRTCConnectionMethod;
use crossbeam_channel::bounded;
use numpy::{PyArray1, PyArrayMethods};
use pyo3::exceptions::{PyNotImplementedError, PyRuntimeError, PyValueError};
use pyo3::prelude::*;
use pyo3::types::{PyAny, PyBool, PyDict, PyList, PyTuple};
use serde_json::{Map, Number, Value};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::thread;
use tokio::sync::Mutex as AsyncMutex;
use tracing::{info, warn};

type CallbackRegistry = Arc<Mutex<HashMap<String, Py<PyAny>>>>;

#[pyclass(name = "WebRTCConnectionMethod")]
pub struct PyWebRTCConnectionMethod;

#[allow(non_upper_case_globals)]
#[pymethods]
impl PyWebRTCConnectionMethod {
    #[classattr]
    const LocalAP: i32 = 1;
    #[classattr]
    const LocalSTA: i32 = 2;
    #[classattr]
    const Remote: i32 = 3;
}

#[pyclass(name = "VUI_COLOR")]
pub struct PyVuiColor;

#[allow(non_upper_case_globals)]
#[pymethods]
impl PyVuiColor {
    #[classattr]
    const WHITE: &'static str = "white";
    #[classattr]
    const RED: &'static str = "red";
    #[classattr]
    const YELLOW: &'static str = "yellow";
    #[classattr]
    const BLUE: &'static str = "blue";
    #[classattr]
    const GREEN: &'static str = "green";
    #[classattr]
    const CYAN: &'static str = "cyan";
    #[classattr]
    const PURPLE: &'static str = "purple";
}

#[pyclass(name = "PubSubBridge", unsendable)]
pub struct PyPubSubBridge {
    service: Arc<DataChannelService<RtcEngine>>,
    callback_registry: CallbackRegistry,
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

#[pyclass(name = "VideoBridge", unsendable)]
pub struct PyVideoBridge {
    datachannel_service: Arc<DataChannelService<RtcEngine>>,
    callback_registry: CallbackRegistry,
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

#[pyclass(name = "AudioBridge", unsendable)]
pub struct PyAudioBridge {
    datachannel_service: Arc<DataChannelService<RtcEngine>>,
    callback_registry: CallbackRegistry,
    rtc_engine: Arc<RtcEngine>,
    audio_sender: Arc<AsyncMutex<Option<crate::application::audio_sender_service::AudioSender>>>,
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

            let source = crate::application::audio_sender_service::AudioSource::File(path);
            let sender =
                crate::application::audio_sender_service::AudioSender::new(source, audio_track)
                    .map_err(to_py_error)?;

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

            let source = crate::application::audio_sender_service::AudioSource::Url(url);
            let sender =
                crate::application::audio_sender_service::AudioSender::new(source, audio_track)
                    .map_err(to_py_error)?;

            sender.play().map_err(to_py_error)?;

            let mut guard = audio_sender_arc.lock().await;
            *guard = Some(sender);

            Ok(())
        })
    }

    #[pyo3(name = "stop")]
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

#[pyclass(name = "DataChannelBridge", unsendable)]
pub struct PyDataChannelBridge {
    service: Arc<DataChannelService<RtcEngine>>,
    callback_registry: CallbackRegistry,
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

    fn set_decoder(&self, decoder_type: &str) -> PyResult<()> {
        self.service
            .set_decoder(decoder_type)
            .map_err(to_py_error)?;
        info!(
            event = "decoder_switch",
            decoder = self.service.decoder_name(),
            "Decoder switched"
        );
        Ok(())
    }

    #[pyo3(signature = (timeout=5.0))]
    fn wait_datachannel_open<'py>(
        &self,
        py: Python<'py>,
        timeout: f64,
    ) -> PyResult<Bound<'py, PyAny>> {
        let service = Arc::clone(&self.service);
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            service
                .wait_datachannel_open(timeout)
                .await
                .map_err(to_py_error)?;
            Ok(())
        })
    }
}

#[pyclass(name = "UnitreeWebRTCConnection", unsendable)]
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
    audio_sender: Arc<AsyncMutex<Option<crate::application::audio_sender_service::AudioSender>>>,
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

        let (incoming_tx, incoming_rx) = bounded::<DcMessage>(1024);
        let (callback_events_tx, callback_events_rx) = bounded::<CallbackEvent>(1024);
        let callback_registry: CallbackRegistry = Arc::new(Mutex::new(HashMap::new()));
        spawn_callback_dispatcher(callback_events_rx, Arc::clone(&callback_registry));
        let lidar_worker_pool = lidar_service::create_worker_pool(callback_events_tx.clone());
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

fn spawn_callback_dispatcher(
    callback_events_rx: crossbeam_channel::Receiver<CallbackEvent>,
    callback_registry: CallbackRegistry,
) {
    thread::Builder::new()
        .name("unitree-python-dispatcher".to_string())
        .spawn(move || {
            while let Ok(event) = callback_events_rx.recv() {
                match event {
                    CallbackEvent::TopicCallback { topic, payload } => {
                        let _ = Python::try_attach(|py| {
                            let callback = {
                                let registry = callback_registry.lock().unwrap();
                                registry.get(&topic).map(|value| value.clone_ref(py))
                            };

                            if let Some(callback) = callback {
                                if let Ok(argument) = json_value_to_py(py, &payload) {
                                    let _ = callback.call1(py, (argument,));
                                }
                            }
                        });
                    }
                    CallbackEvent::LidarCallback {
                        topic,
                        payload,
                        points,
                    } => {
                        let _ = Python::try_attach(|py| {
                            let callback = {
                                let registry = callback_registry.lock().unwrap();
                                registry.get(&topic).map(|value| value.clone_ref(py))
                            };

                            if let Some(callback) = callback {
                                let point_count = points.len() / 3;

                                // points already Vec<f64> from worker pool (no conversion needed!)
                                let numpy_array = PyArray1::from_vec(py, points);

                                if let Ok(reshaped) = numpy_array.reshape([point_count, 3]) {
                                    // Build minimal message dict (no JSON serialization!)
                                    let message = PyDict::new(py);
                                    let _ = message.set_item("topic", &topic);

                                    // Extract only essential metadata (no json_value_to_py!)
                                    if let Some(data_obj) =
                                        payload.get("data").and_then(|v| v.as_object())
                                    {
                                        let data_dict = PyDict::new(py);

                                        // Copy scalar fields directly
                                        if let Some(stamp) =
                                            data_obj.get("stamp").and_then(|v| v.as_f64())
                                        {
                                            let _ = data_dict.set_item("stamp", stamp);
                                        }
                                        if let Some(frame_id) =
                                            data_obj.get("frame_id").and_then(|v| v.as_str())
                                        {
                                            let _ = data_dict.set_item("frame_id", frame_id);
                                        }
                                        if let Some(resolution) =
                                            data_obj.get("resolution").and_then(|v| v.as_f64())
                                        {
                                            let _ = data_dict.set_item("resolution", resolution);
                                        }
                                        if let Some(src_size) =
                                            data_obj.get("src_size").and_then(|v| v.as_u64())
                                        {
                                            let _ = data_dict.set_item("src_size", src_size);
                                        }
                                        if let Some(width) =
                                            data_obj.get("width").and_then(|v| v.as_u64())
                                        {
                                            let _ = data_dict.set_item("width", width);
                                        }
                                        if let Some(origin) =
                                            data_obj.get("origin").and_then(|v| v.as_array())
                                        {
                                            let origin_list = origin
                                                .iter()
                                                .filter_map(|v| v.as_f64())
                                                .collect::<Vec<_>>();
                                            let _ = data_dict.set_item("origin", origin_list);
                                        }

                                        // Add points (already numpy array)
                                        let points_dict = PyDict::new(py);
                                        let _ = points_dict.set_item("points", reshaped.as_any());
                                        let _ = data_dict.set_item("data", points_dict);

                                        let _ = message.set_item("data", data_dict);
                                    }

                                    if let Some(msg_type) =
                                        payload.get("type").and_then(|v| v.as_str())
                                    {
                                        let _ = message.set_item("type", msg_type);
                                    }

                                    let _ = callback.call1(py, (message,));
                                }
                            }
                        });
                    }
                    CallbackEvent::VideoFrame {
                        data,
                        width,
                        height,
                    } => {
                        let _ = Python::try_attach(|py| {
                            let callback = {
                                let registry = callback_registry.lock().unwrap();
                                registry.get("video").map(|value| value.clone_ref(py))
                            };

                            if let Some(callback) = callback {
                                let h = height as usize;
                                let w = width as usize;

                                // Zero-copy: flat Vec → reshape to (H, W, 3)
                                let numpy_flat = PyArray1::from_vec(py, data);
                                if let Ok(numpy_frame) = numpy_flat.reshape([h, w, 3]) {
                                    let _ = callback.call1(py, (numpy_frame.as_any(),));
                                }
                            }
                        });
                    }
                    CallbackEvent::AudioFrame {
                        data,
                        sample_rate: _,
                        channels: _,
                    } => {
                        let _ = Python::try_attach(|py| {
                            let callback = {
                                let registry = callback_registry.lock().unwrap();
                                registry.get("audio").map(|value| value.clone_ref(py))
                            };

                            if let Some(callback) = callback {
                                let numpy_samples = PyArray1::from_vec(py, data);
                                let _ = callback.call1(py, (numpy_samples.as_any(),));
                            }
                        });
                    }
                    CallbackEvent::FutureResolve { .. } => {
                        warn!(
                            event = "callback_event_unused",
                            kind = "future_resolve",
                            "FutureResolve callback event is currently ignored"
                        );
                    }
                }
            }
        })
        .expect("failed to spawn python callback dispatcher");
}

fn py_any_to_json_value(object: &Bound<'_, PyAny>) -> PyResult<Value> {
    if object.is_none() {
        return Ok(Value::Null);
    }

    if let Ok(value) = object.extract::<bool>() {
        return Ok(Value::Bool(value));
    }

    if let Ok(value) = object.extract::<i64>() {
        return Ok(Value::Number(Number::from(value)));
    }

    if let Ok(value) = object.extract::<u64>() {
        return Ok(Value::Number(Number::from(value)));
    }

    if let Ok(value) = object.extract::<f64>() {
        let number = Number::from_f64(value)
            .ok_or_else(|| PyValueError::new_err("Float value must be finite"))?;
        return Ok(Value::Number(number));
    }

    if let Ok(value) = object.extract::<String>() {
        return Ok(Value::String(value));
    }

    if let Ok(dict) = object.cast::<PyDict>() {
        let mut output = Map::new();
        for (key, value) in dict.iter() {
            let key_string = key.extract::<String>()?;
            output.insert(key_string, py_any_to_json_value(&value)?);
        }
        return Ok(Value::Object(output));
    }

    if let Ok(list) = object.cast::<PyList>() {
        let mut output = Vec::with_capacity(list.len());
        for item in list.iter() {
            output.push(py_any_to_json_value(&item)?);
        }
        return Ok(Value::Array(output));
    }

    if let Ok(tuple) = object.cast::<PyTuple>() {
        let mut output = Vec::with_capacity(tuple.len());
        for item in tuple.iter() {
            output.push(py_any_to_json_value(&item)?);
        }
        return Ok(Value::Array(output));
    }

    if let Ok(bytes) = object.extract::<Vec<u8>>() {
        let output = bytes
            .into_iter()
            .map(|byte| Value::Number(Number::from(byte)))
            .collect::<Vec<_>>();
        return Ok(Value::Array(output));
    }

    Err(PyValueError::new_err(
        "Unsupported Python value type for JSON conversion",
    ))
}

fn json_value_to_py(py: Python<'_>, value: &Value) -> PyResult<Py<PyAny>> {
    match value {
        Value::Null => Ok(py.None()),
        Value::Bool(v) => Ok(PyBool::new(py, *v).to_owned().into_any().unbind()),
        Value::Number(number) => {
            if let Some(value) = number.as_i64() {
                return Ok(value.into_pyobject(py)?.into_any().unbind());
            }
            if let Some(value) = number.as_u64() {
                return Ok(value.into_pyobject(py)?.into_any().unbind());
            }
            if let Some(value) = number.as_f64() {
                return Ok(value.into_pyobject(py)?.into_any().unbind());
            }
            Err(PyValueError::new_err("Unsupported JSON number"))
        }
        Value::String(text) => Ok(text.into_pyobject(py)?.into_any().unbind()),
        Value::Array(items) => {
            let list = PyList::empty(py);
            for item in items {
                list.append(json_value_to_py(py, item)?)?;
            }
            Ok(list.into_any().unbind())
        }
        Value::Object(map) => {
            let dict = PyDict::new(py);
            for (key, item) in map {
                dict.set_item(key, json_value_to_py(py, item)?)?;
            }
            Ok(dict.into_any().unbind())
        }
    }
}

fn to_py_error(error: String) -> PyErr {
    if error.contains("not implemented") {
        return PyNotImplementedError::new_err(error);
    }

    PyRuntimeError::new_err(error)
}

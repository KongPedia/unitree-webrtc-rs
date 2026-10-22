use crate::diagnostics::RepeatLog;
use crate::interface::bridges::pubsub::CallbackRegistry;
use crate::interface::utils::json_value_to_py;
use crate::protocol::events::{EventDelivery, EventReceiver};
use crate::protocol::models::CallbackEvent;
use numpy::{PyArray1, PyArrayMethods};
use pyo3::prelude::*;
use pyo3::types::PyDict;
use std::thread;
use tracing::warn;

pub fn spawn_callback_dispatcher(
    callback_events_rx: EventReceiver,
    callback_registry: CallbackRegistry,
) {
    thread::Builder::new()
        .name("unitree-python-dispatcher".to_string())
        .spawn(move || {
            let failures = CallbackDiagnostics::new();
            let mut generation = None;
            while let Ok(delivery) = callback_events_rx.recv_delivery() {
                if generation != Some(delivery.generation()) {
                    generation = Some(delivery.generation());
                    failures.reset(delivery.generation());
                }
                dispatch_delivery(delivery, &callback_registry, &failures);
            }
        })
        .expect("failed to spawn python callback dispatcher");
}

fn dispatch_delivery(
    mut delivery: EventDelivery,
    registry: &CallbackRegistry,
    failures: &CallbackDiagnostics,
) {
    let event = delivery.take_event();
    dispatch_event_checked(event, registry, &|| delivery.is_current(), failures);
}

#[cfg(test)]
fn dispatch_event(event: CallbackEvent, registry: &CallbackRegistry) {
    dispatch_event_checked(event, registry, &|| true, &CallbackDiagnostics::new());
}

fn dispatch_event_checked(
    event: CallbackEvent,
    registry: &CallbackRegistry,
    current: &dyn Fn() -> bool,
    failures: &CallbackDiagnostics,
) {
    match event {
        CallbackEvent::TopicCallback { topic, payload } => {
            dispatch_callback(registry, &topic, "topic", current, failures, |py| {
                json_value_to_py(py, &payload)
            });
        }
        CallbackEvent::LidarCallback {
            topic,
            payload,
            points,
        } => {
            dispatch_callback(registry, &topic, "lidar", current, failures, |py| {
                let point_count = points.len() / 3;
                // Transfer the owned Vec<f32> to NumPy; retained arrays own its lifetime.
                let numpy_array = PyArray1::from_vec(py, points);
                let reshaped = numpy_array.reshape([point_count, 3])?;
                let message = PyDict::new(py);
                message.set_item("topic", &topic)?;
                if let Some(data_obj) = payload.get("data").and_then(|v| v.as_object()) {
                    let data_dict = PyDict::new(py);
                    if let Some(stamp) = data_obj.get("stamp").and_then(|v| v.as_f64()) {
                        data_dict.set_item("stamp", stamp)?;
                    }
                    if let Some(frame_id) = data_obj.get("frame_id").and_then(|v| v.as_str()) {
                        data_dict.set_item("frame_id", frame_id)?;
                    }
                    if let Some(resolution) = data_obj.get("resolution").and_then(|v| v.as_f64()) {
                        data_dict.set_item("resolution", resolution)?;
                    }
                    if let Some(src_size) = data_obj.get("src_size").and_then(|v| v.as_u64()) {
                        data_dict.set_item("src_size", src_size)?;
                    }
                    if let Some(width) = data_obj.get("width").and_then(|v| v.as_u64()) {
                        data_dict.set_item("width", width)?;
                    }
                    if let Some(origin) = data_obj.get("origin").and_then(|v| v.as_array()) {
                        let origin_list =
                            origin.iter().filter_map(|v| v.as_f64()).collect::<Vec<_>>();
                        data_dict.set_item("origin", origin_list)?;
                    }
                    let points_dict = PyDict::new(py);
                    points_dict.set_item("points", reshaped.as_any())?;
                    data_dict.set_item("data", points_dict)?;
                    message.set_item("data", data_dict)?;
                }
                if let Some(msg_type) = payload.get("type").and_then(|v| v.as_str()) {
                    message.set_item("type", msg_type)?;
                }
                Ok(message.into_any().unbind())
            });
        }
        CallbackEvent::VideoFrame {
            data,
            width,
            height,
        } => {
            dispatch_callback(registry, "video", "video", current, failures, |py| {
                // Owned Rust buffer → NumPy; reshape does not copy the frame.
                let numpy_flat = PyArray1::from_vec(py, data);
                Ok(numpy_flat
                    .reshape([height as usize, width as usize, 3])?
                    .into_any()
                    .unbind())
            });
        }
        CallbackEvent::AudioFrame { data, .. } => {
            dispatch_callback(registry, "audio", "audio", current, failures, |py| {
                Ok(PyArray1::from_vec(py, data).into_any().unbind())
            });
        }
        CallbackEvent::FutureResolve { .. } => warn!(
            event = "callback_event_unused",
            kind = "future_resolve",
            "FutureResolve callback event is currently ignored"
        ),
    }
}

fn dispatch_callback(
    registry: &CallbackRegistry,
    key: &str,
    kind: &'static str,
    current: &dyn Fn() -> bool,
    failures: &CallbackDiagnostics,
    argument: impl FnOnce(Python<'_>) -> PyResult<Py<PyAny>>,
) {
    let attached = Python::try_attach(|py| {
        if !current() {
            return;
        }
        let callback = {
            let registry = registry.lock().unwrap();
            registry
                .get(key)
                .map(|value| (value.id, value.callback.clone_ref(py)))
        };
        let Some((id, callback)) = callback else {
            return;
        };
        let argument = match argument(py) {
            Ok(argument) => argument,
            Err(error) => {
                failures.report(py, &error, "conversion", kind, id);
                return;
            }
        };
        // Admission occurs after attachment/conversion; running callbacks may finish after reset.
        if !current() {
            return;
        }
        if let Err(error) = callback.call1(py, (argument,)) {
            failures.report(py, &error, "invocation", kind, id);
        }
    });
    if attached.is_none() {
        failures.attachment.record();
    }
}

const ERROR_CLASSES: [&str; 9] = [
    "ValueError",
    "TypeError",
    "RuntimeError",
    "KeyError",
    "IndexError",
    "MemoryError",
    "OverflowError",
    "OSError",
    "unknown_exception",
];

struct CallbackDiagnostics {
    conversion: [RepeatLog; 9],
    invocation: [RepeatLog; 9],
    attachment: RepeatLog,
    generation: std::sync::atomic::AtomicU64,
}

impl CallbackDiagnostics {
    fn new() -> Self {
        Self {
            conversion: std::array::from_fn(|i| {
                RepeatLog::new("python_callback_conversion_failed", ERROR_CLASSES[i])
            }),
            invocation: std::array::from_fn(|i| {
                RepeatLog::new("python_callback_invocation_failed", ERROR_CLASSES[i])
            }),
            attachment: RepeatLog::new("python_callback_failed", "interpreter_unavailable"),
            generation: std::sync::atomic::AtomicU64::new(0),
        }
    }

    fn reset(&self, generation: u64) {
        for counter in self
            .conversion
            .iter()
            .chain(self.invocation.iter())
            .chain(std::iter::once(&self.attachment))
        {
            counter.reset();
        }
        self.generation
            .store(generation, std::sync::atomic::Ordering::Relaxed);
    }

    fn report(
        &self,
        py: Python<'_>,
        error: &PyErr,
        stage: &'static str,
        kind: &'static str,
        id: u64,
    ) {
        let class = safe_exception_class(py, error);
        let index = ERROR_CLASSES
            .iter()
            .position(|value| *value == class)
            .unwrap();
        let counters = if stage == "conversion" {
            &self.conversion
        } else {
            &self.invocation
        };
        counters[index].record_with(|count, suppressed_count| {
            warn!(
                event = "python_callback_failed",
                stage,
                last_callback_kind = kind,
                last_callback_id = id,
                error_class = class,
                scope = "dispatcher",
                count,
                suppressed_count,
                generation = self.generation.load(std::sync::atomic::Ordering::Relaxed),
                "Callback failure; exception text and payload omitted"
            );
        });
    }
}

fn safe_exception_class(py: Python<'_>, error: &PyErr) -> &'static str {
    use pyo3::exceptions::*;
    let actual = error.get_type(py);
    let known = [
        py.get_type::<PyValueError>(),
        py.get_type::<PyTypeError>(),
        py.get_type::<PyRuntimeError>(),
        py.get_type::<PyKeyError>(),
        py.get_type::<PyIndexError>(),
        py.get_type::<PyMemoryError>(),
        py.get_type::<PyOverflowError>(),
        py.get_type::<PyOSError>(),
    ];
    known
        .iter()
        .position(|expected| actual.is(expected))
        .map_or("unknown_exception", |i| ERROR_CLASSES[i])
}

#[cfg(test)]
mod tests {
    use super::*;
    use pyo3::exceptions::PyValueError;
    use pyo3::types::{PyCFunction, PyModule};
    use serde_json::json;
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};

    #[test]
    fn dequeued_event_waiting_for_python_cannot_enter_a_new_generation() {
        Python::initialize();
        Python::attach(|py| {
            let module = PyModule::from_code(
                py,
                c"received=[]\ndef callback(value): received.append(value)",
                c"epoch_test.py",
                c"epoch_test",
            )
            .unwrap();
            let registry: CallbackRegistry = Arc::new(Mutex::new(HashMap::new()));
            registry.lock().unwrap().insert(
                "topic".into(),
                module.getattr("callback").unwrap().unbind().into(),
            );
            let (sender, receiver) = crate::protocol::events::event_channel(1).unwrap();
            sender
                .try_send(CallbackEvent::TopicCallback {
                    topic: "topic".into(),
                    payload: json!({}),
                })
                .unwrap();
            let delivery = receiver.recv_delivery().unwrap();
            let (started, waiting) = std::sync::mpsc::channel();
            let worker = std::thread::spawn(move || {
                started.send(()).unwrap();
                dispatch_delivery(delivery, &registry, &CallbackDiagnostics::new());
            });
            waiting
                .recv_timeout(std::time::Duration::from_secs(2))
                .unwrap();
            // This thread still holds Python; the delivery has already left the queue.
            sender.set_generation(1);
            py.detach(|| worker.join().unwrap());
            assert_eq!(module.getattr("received").unwrap().len().unwrap(), 0);
        });
    }

    #[test]
    fn callback_admission_is_rechecked_after_argument_conversion() {
        Python::initialize();
        Python::attach(|py| {
            let module = PyModule::from_code(
                py,
                c"received=[]\ndef callback(value): received.append(value)",
                c"conversion_epoch.py",
                c"conversion_epoch",
            )
            .unwrap();
            let registry: CallbackRegistry = Arc::new(Mutex::new(HashMap::new()));
            registry.lock().unwrap().insert(
                "topic".into(),
                module.getattr("callback").unwrap().unbind().into(),
            );
            let current = AtomicBool::new(true);
            dispatch_callback(
                &registry,
                "topic",
                "topic",
                &|| current.load(Ordering::SeqCst),
                &CallbackDiagnostics::new(),
                |py| {
                    current.store(false, Ordering::SeqCst);
                    Ok(py.None())
                },
            );
            assert_eq!(module.getattr("received").unwrap().len().unwrap(), 0);
        });
    }

    #[test]
    fn exception_classification_uses_identity_not_secret_bearing_class_names() {
        Python::initialize();
        Python::attach(|py| {
            assert_eq!(
                safe_exception_class(py, &PyValueError::new_err("secret message")),
                "ValueError"
            );
            let custom = py
                .eval(
                    c"type('secret_canary', (ValueError,), {})('secret message')",
                    None,
                    None,
                )
                .unwrap();
            assert_eq!(
                safe_exception_class(py, &PyErr::from_value(custom)),
                "unknown_exception"
            );
        });
    }

    #[test]
    fn callback_runs_without_registry_lock_and_failure_does_not_stop_dispatch() {
        Python::initialize();
        let registry: CallbackRegistry = Arc::new(Mutex::new(HashMap::new()));
        let invoked = Arc::new(AtomicBool::new(false));
        Python::attach(|py| {
            let callback_invoked = Arc::clone(&invoked);
            let checked_registry = Arc::clone(&registry);
            let callback = PyCFunction::new_closure(py, None, None, move |_, _| -> PyResult<()> {
                assert!(checked_registry.try_lock().is_ok());
                callback_invoked.store(true, Ordering::Relaxed);
                Err(PyValueError::new_err("sensitive user exception text"))
            })
            .unwrap();
            registry
                .lock()
                .unwrap()
                .insert("test".into(), callback.into_any().unbind().into());
        });
        dispatch_event(
            CallbackEvent::TopicCallback {
                topic: "test".into(),
                payload: json!({}),
            },
            &registry,
        );
        assert!(invoked.load(Ordering::Relaxed));
        Python::attach(|py| {
            let module = PyModule::from_code(
                py,
                c"received = []\ndef callback(value): received.append(value)",
                c"callback_recovery_test.py",
                c"callback_recovery_test",
            )
            .unwrap();
            registry.lock().unwrap().insert(
                "test".into(),
                module.getattr("callback").unwrap().unbind().into(),
            );
            dispatch_event(
                CallbackEvent::TopicCallback {
                    topic: "test".into(),
                    payload: json!({"value": 7}),
                },
                &registry,
            );
            assert_eq!(module.getattr("received").unwrap().len().unwrap(), 1);
        });
    }

    #[test]
    fn media_arrays_retain_owned_buffers_and_expected_dtype_shape() {
        Python::initialize();
        Python::attach(|py| {
            let module = PyModule::from_code(
                py,
                c"received = []\ndef callback(value): received.append(value)",
                c"owned_media_test.py",
                c"owned_media_test",
            )
            .unwrap();
            let registry: CallbackRegistry = Arc::new(Mutex::new(HashMap::new()));
            for key in ["video", "audio", "lidar"] {
                registry.lock().unwrap().insert(
                    key.into(),
                    module.getattr("callback").unwrap().unbind().into(),
                );
            }
            // Invalid reshape must not deliver a partial frame or stop later events.
            dispatch_event(
                CallbackEvent::VideoFrame {
                    data: vec![1],
                    width: 1,
                    height: 1,
                },
                &registry,
            );
            let video_data = vec![1, 2, 3];
            module
                .setattr("video_pointer", video_data.as_ptr() as usize)
                .unwrap();
            dispatch_event(
                CallbackEvent::VideoFrame {
                    data: video_data,
                    width: 1,
                    height: 1,
                },
                &registry,
            );
            dispatch_event(
                CallbackEvent::AudioFrame {
                    data: vec![4, 5],
                    sample_rate: 48000,
                    channels: 1,
                },
                &registry,
            );
            let points = vec![1.0_f32, 2.0, 3.0];
            module
                .setattr("points_pointer", points.as_ptr() as usize)
                .unwrap();
            dispatch_event(
                CallbackEvent::LidarCallback {
                    topic: "lidar".into(),
                    payload: json!({"data": {}}),
                    points,
                },
                &registry,
            );
            drop(registry);
            py.run(c"video, audio, message = received\npoints = message['data']['data']['points']\nassert video.dtype.name == 'uint8' and video.shape == (1, 1, 3)\nassert video.tolist() == [[[1, 2, 3]]]\nassert video.ctypes.data == video_pointer\nassert audio.dtype.name == 'int16' and audio.tolist() == [4, 5]\nassert points.dtype.name == 'float32' and points.shape == (1, 3)\nassert points.tolist() == [[1.0, 2.0, 3.0]]\nassert points.ctypes.data == points_pointer", Some(&module.dict()), None).unwrap();
        });
    }
}

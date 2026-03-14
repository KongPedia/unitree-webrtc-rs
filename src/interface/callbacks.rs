use crate::interface::bridges::pubsub::CallbackRegistry;
use crate::interface::utils::json_value_to_py;
use crate::protocol::models::CallbackEvent;
use numpy::{PyArray1, PyArrayMethods};
use pyo3::prelude::*;
use pyo3::types::PyDict;
use std::thread;
use tracing::warn;

pub fn spawn_callback_dispatcher(
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

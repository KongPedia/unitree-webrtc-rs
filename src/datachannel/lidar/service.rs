use super::codec::{
    bits_to_points_optimized, decompress_lz4, validate_geometry, MAX_COMPRESSED_BYTES,
};
use crate::diagnostics::RepeatLog;
use crate::protocol::events::EventSender;
use crate::protocol::models::CallbackEvent;
use bytes::Bytes;
use crossbeam_channel::{bounded, Sender, TrySendError};
use serde_json::Value;
use std::sync::Arc;
use std::thread;

pub struct LidarDecodeRequest {
    pub generation: u64,
    pub topic: String,
    pub payload: Value,
    pub compressed_data: Bytes,
    pub metadata: LidarMetadata,
}

pub struct LidarMetadata {
    pub origin: [f64; 3],
    pub resolution: f64,
    pub src_size: usize,
}

impl LidarMetadata {
    pub fn from_json(data: &Value) -> Option<Self> {
        let data_obj = data.as_object()?;
        let origin_array = data_obj.get("origin")?.as_array()?;

        if origin_array.len() != 3 {
            return None;
        }
        let origin = [
            origin_array.first()?.as_f64()?,
            origin_array.get(1)?.as_f64()?,
            origin_array.get(2)?.as_f64()?,
        ];

        let resolution = data.get("resolution")?.as_f64()?;
        let src_size = usize::try_from(data.get("src_size")?.as_u64()?).ok()?;
        validate_geometry(src_size, &origin, resolution).ok()?;

        Some(Self {
            origin,
            resolution,
            src_size,
        })
    }
}

pub struct LidarWorkerPool {
    request_tx: Sender<LidarDecodeRequest>,
}

impl LidarWorkerPool {
    pub fn new(callback_events_tx: EventSender, worker_count: usize) -> Self {
        // Compressed inputs are bounded separately; do not retain a burst of 128 MiB.
        let (request_tx, request_rx) = bounded::<LidarDecodeRequest>(4);

        for worker_id in 0..worker_count {
            let rx = request_rx.clone();
            let callback_tx = callback_events_tx.clone();

            thread::Builder::new()
                .name(format!("unitree-lidar-worker-{worker_id}"))
                .spawn(move || {
                    let dropped = RepeatLog::new("lidar_callback_drop", "stale_or_backpressure");
                    let disconnected =
                        RepeatLog::new("lidar_callback_drop", "dispatcher_disconnected");
                    let failed = RepeatLog::new("lidar_decode_failed", "invalid_lidar_data");
                    let mut generation = None;
                    while let Ok(req) = rx.recv() {
                        if generation != Some(req.generation) {
                            for log in [&dropped, &disconnected, &failed] {
                                log.reset();
                            }
                            generation = Some(req.generation);
                        }
                        let sender = callback_tx.for_generation(req.generation);
                        if sender
                            .generation_state()
                            .load(std::sync::atomic::Ordering::Acquire)
                            != req.generation
                        {
                            dropped.record();
                            continue;
                        }
                        match Self::decode_lidar(&req) {
                            Ok(points) => {
                                let event = CallbackEvent::LidarCallback {
                                    topic: req.topic,
                                    payload: req.payload,
                                    points,
                                };
                                match sender.try_send(event) {
                                    Ok(()) => {}
                                    Err(TrySendError::Full(_)) => dropped.record(),
                                    Err(TrySendError::Disconnected(_)) => disconnected.record(),
                                }
                            }
                            Err(_) => failed.record(),
                        }
                    }
                })
                .expect("failed to spawn lidar worker thread");
        }

        Self { request_tx }
    }

    pub fn submit(&self, request: LidarDecodeRequest) -> Result<(), String> {
        validate_geometry(
            request.metadata.src_size,
            &request.metadata.origin,
            request.metadata.resolution,
        )?;
        if request.compressed_data.len() > MAX_COMPRESSED_BYTES {
            return Err("LiDAR compressed size exceeds resource budget".into());
        }
        self.request_tx.try_send(request).map_err(|e| match e {
            TrySendError::Full(_) => "LiDAR worker queue full".to_string(),
            TrySendError::Disconnected(_) => "LiDAR workers disconnected".to_string(),
        })
    }

    fn decode_lidar(req: &LidarDecodeRequest) -> Result<Vec<f32>, String> {
        validate_geometry(
            req.metadata.src_size,
            &req.metadata.origin,
            req.metadata.resolution,
        )?;
        let decompressed = decompress_lz4(&req.compressed_data, req.metadata.src_size)?;

        if decompressed.len() != req.metadata.src_size {
            return Err(format!(
                "Decompressed size {} != expected {}",
                decompressed.len(),
                req.metadata.src_size
            ));
        }

        // Use optimized decoder path
        let points =
            bits_to_points_optimized(&decompressed, &req.metadata.origin, req.metadata.resolution)?;

        Ok(points)
    }
}

impl Clone for LidarWorkerPool {
    fn clone(&self) -> Self {
        Self {
            request_tx: self.request_tx.clone(),
        }
    }
}

pub fn create_worker_pool(callback_events_tx: EventSender) -> Arc<LidarWorkerPool> {
    // Use CPU count for optimal parallelism, with min 2 and max 4
    let worker_count = std::thread::available_parallelism()
        .map(|n| n.get().clamp(2, 4))
        .unwrap_or(2);

    Arc::new(LidarWorkerPool::new(callback_events_tx, worker_count))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::events::event_channel;
    use serde_json::json;

    // ── LidarMetadata ─────────────────────────────────────────────────────────

    /// Valid JSON with origin, resolution, src_size parses successfully.
    #[test]
    fn lidar_metadata_parses_valid_json() {
        let data = json!({
            "origin": [-5.12, -5.12, -0.64],
            "resolution": 0.04,
            "src_size": 131072
        });
        let meta = LidarMetadata::from_json(&data).expect("should parse");
        assert_eq!(meta.origin, [-5.12, -5.12, -0.64]);
        assert!((meta.resolution - 0.04).abs() < 1e-9);
        assert_eq!(meta.src_size, 131072);
    }

    /// Missing any required field returns None.
    #[test]
    fn lidar_metadata_returns_none_on_missing_fields() {
        // missing src_size
        let missing_src = json!({"origin": [0.0, 0.0, 0.0], "resolution": 0.04});
        assert!(LidarMetadata::from_json(&missing_src).is_none());

        // missing origin
        let missing_origin = json!({"resolution": 0.04, "src_size": 1024});
        assert!(LidarMetadata::from_json(&missing_origin).is_none());

        // empty object
        assert!(LidarMetadata::from_json(&json!({})).is_none());
    }

    // ── LidarWorkerPool ───────────────────────────────────────────────────────

    /// Submitting a synthetic 1-bit buffer produces xyz triplets via the worker pool.
    #[test]
    fn worker_pool_decodes_single_bit_to_xyz_triplet() {
        let (cb_tx, cb_rx) = event_channel(8).unwrap();
        let pool = create_worker_pool(cb_tx);

        // Use lidar_codec::bits_to_points directly to know expected output
        let mut decompressed = vec![0u8; 3];
        decompressed[0] = 0b0000_0001; // bit 7 of byte 0 → x=7, y=0, z=0
        let origin = [0.0f64, 0.0, 0.0];
        let resolution = 0.05;

        // We need to produce the compressed form that decompresses to `decompressed`.
        // lz4_flex's block format: just use the raw bytes as content via uncompressed sentinel.
        // Instead, submit directly using the internal decode_lidar by crafting a request
        // where compressed_data decompresses to our known buffer.
        let compressed = lz4_flex::block::compress(&decompressed);

        let req = LidarDecodeRequest {
            generation: 0,
            topic: "rt/utlidar/voxel_map_compressed".to_string(),
            payload: json!({"data": {}}),
            compressed_data: Bytes::from(compressed),
            metadata: LidarMetadata {
                origin,
                resolution,
                src_size: 3,
            },
        };

        pool.submit(req).expect("submit should succeed");

        // Wait for the worker thread to decode and emit callback
        let event = cb_rx
            .recv_timeout(std::time::Duration::from_secs(2))
            .expect("expected LidarCallback within 2s");

        match event {
            CallbackEvent::LidarCallback {
                topic,
                points,
                payload: _,
            } => {
                assert_eq!(topic, "rt/utlidar/voxel_map_compressed");
                // bit 0 of byte 0 (MSB of first byte, value 0b0000_0001 → bit 7 zero-indexed from MSB is at position 0)
                // bits_to_points: x = (n_slice % 0x10) * 8 + bit_offset = 0*8+7 = 7
                // y = n_slice / 0x10 = 0, z = byte_index / 0x800 = 0
                // px = 7 * 0.05 + 0.0 = 0.35
                assert_eq!(points.len(), 3, "should produce one xyz triplet");
                assert!((points[0] - 0.35).abs() < 1e-9, "x = 7 * 0.05 = 0.35");
                assert!((points[1] - 0.0).abs() < 1e-9, "y = 0");
                assert!((points[2] - 0.0).abs() < 1e-9, "z = 0");
            }
            other => panic!("Expected LidarCallback, got {:?}", other),
        }
    }

    /// Worker pool submit() correctly accepts valid requests.
    #[test]
    fn worker_pool_submit_accepts_valid_requests() {
        let (cb_tx, _cb_rx) = event_channel(8).unwrap();
        let pool = create_worker_pool(cb_tx);

        let data = vec![0u8; 3];
        let compressed = lz4_flex::block::compress(&data);

        let req = LidarDecodeRequest {
            generation: 0,
            topic: "rt/utlidar/voxel_map_compressed".to_string(),
            payload: json!({}),
            compressed_data: compressed.into(),
            metadata: LidarMetadata {
                origin: [0.0; 3],
                resolution: 0.05,
                src_size: 3,
            },
        };

        let result = pool.submit(req);
        assert!(result.is_ok(), "valid request should be accepted");
    }
}

#[cfg(test)]
mod bounds_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn metadata_rejects_invalid_geometry_and_size() {
        for data in [
            json!({"origin":[0,0,0],"resolution":0,"src_size":3}),
            json!({"origin":[0,0,0],"resolution":-0.1,"src_size":3}),
            json!({"origin":[0,0,0,1],"resolution":0.1,"src_size":3}),
            json!({"origin":[1e300,0,0],"resolution":0.1,"src_size":3}),
            json!({"origin":[0,0,0],"resolution":0.1,"src_size":u64::MAX}),
        ] {
            assert!(LidarMetadata::from_json(&data).is_none(), "{data}");
        }
    }
    #[test]
    fn direct_requests_cannot_bypass_metadata_or_compressed_bounds() {
        let (tx, _rx) = bounded(1);
        let pool = LidarWorkerPool { request_tx: tx };
        let request = |compressed_data, resolution| LidarDecodeRequest {
            generation: 0,
            topic: "lidar".into(),
            payload: json!({}),
            compressed_data,
            metadata: LidarMetadata {
                origin: [0.0; 3],
                resolution,
                src_size: 1,
            },
        };
        assert!(pool
            .submit(request(Bytes::from_static(&[1]), f64::NAN))
            .is_err());
        assert!(pool
            .submit(request(
                Bytes::from(vec![0; MAX_COMPRESSED_BYTES + 1]),
                0.04
            ))
            .is_err());
        assert!(
            LidarWorkerPool::decode_lidar(&request(Bytes::from_static(&[1]), f64::INFINITY))
                .is_err()
        );
    }
}

#[cfg(test)]
mod generation_tests {
    use super::*;
    use crate::protocol::events::event_channel;
    use serde_json::json;

    #[test]
    fn old_submitted_work_cannot_be_relabelled_on_completion() {
        let (tx, rx) = event_channel(8).unwrap();
        tx.set_generation(1);
        let pool = LidarWorkerPool::new(tx, 1);
        for generation in [0, 1] {
            pool.submit(LidarDecodeRequest {
                generation,
                topic: generation.to_string(),
                payload: json!({}),
                compressed_data: lz4_flex::block::compress(&[1]).into(),
                metadata: LidarMetadata {
                    origin: [0.0; 3],
                    resolution: 0.05,
                    src_size: 1,
                },
            })
            .unwrap();
        }
        match rx.recv_timeout(std::time::Duration::from_secs(2)).unwrap() {
            CallbackEvent::LidarCallback { topic, .. } => assert_eq!(topic, "1"),
            _ => panic!("expected LiDAR callback"),
        }
        assert!(rx.try_recv().is_err());
    }
}

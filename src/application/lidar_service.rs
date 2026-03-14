use crate::domain::models::CallbackEvent;
use crate::infrastructure::lidar_codec::{bits_to_points, decompress_lz4};
use crossbeam_channel::{bounded, Sender, TrySendError};
use serde_json::Value;
use std::sync::Arc;
use std::thread;
use tracing::warn;

pub struct LidarDecodeRequest {
    pub topic: String,
    pub payload: Value,
    pub compressed_data: Vec<u8>,
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

        let origin = [
            origin_array.first()?.as_f64()?,
            origin_array.get(1)?.as_f64()?,
            origin_array.get(2)?.as_f64()?,
        ];

        let resolution = data.get("resolution")?.as_f64()?;
        let src_size = data.get("src_size")?.as_u64()? as usize;

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
    pub fn new(callback_events_tx: Sender<CallbackEvent>, worker_count: usize) -> Self {
        let (request_tx, request_rx) = bounded::<LidarDecodeRequest>(64);

        for worker_id in 0..worker_count {
            let rx = request_rx.clone();
            let callback_tx = callback_events_tx.clone();

            thread::Builder::new()
                .name(format!("unitree-lidar-worker-{worker_id}"))
                .spawn(move || {
                    while let Ok(req) = rx.recv() {
                        match Self::decode_lidar(&req) {
                            Ok(points) => {
                                let event = CallbackEvent::LidarCallback {
                                    topic: req.topic,
                                    payload: req.payload,
                                    points,
                                };

                                if let Err(error) = callback_tx.try_send(event) {
                                    match error {
                                        TrySendError::Full(_) => {
                                            warn!(
                                                event = "lidar_callback_drop",
                                                worker = worker_id,
                                                reason = "callback_queue_full",
                                                "Dropped LiDAR callback due to backpressure"
                                            );
                                        }
                                        TrySendError::Disconnected(_) => {
                                            warn!(
                                                event = "lidar_callback_drop",
                                                worker = worker_id,
                                                reason = "callback_dispatcher_disconnected",
                                                "Dropped LiDAR callback because dispatcher unavailable"
                                            );
                                        }
                                    }
                                }
                            }
                            Err(error) => {
                                warn!(
                                    event = "lidar_decode_failed",
                                    worker = worker_id,
                                    topic = req.topic,
                                    error = %error,
                                    "Failed to decode LiDAR data"
                                );
                            }
                        }
                    }
                })
                .expect("failed to spawn lidar worker thread");
        }

        Self { request_tx }
    }

    pub fn submit(&self, request: LidarDecodeRequest) -> Result<(), String> {
        self.request_tx.try_send(request).map_err(|e| match e {
            TrySendError::Full(_) => "LiDAR worker queue full".to_string(),
            TrySendError::Disconnected(_) => "LiDAR workers disconnected".to_string(),
        })
    }

    fn decode_lidar(req: &LidarDecodeRequest) -> Result<Vec<f64>, String> {
        let decompressed = decompress_lz4(&req.compressed_data, req.metadata.src_size)?;

        if decompressed.len() != req.metadata.src_size {
            return Err(format!(
                "Decompressed size {} != expected {}",
                decompressed.len(),
                req.metadata.src_size
            ));
        }

        // bits_to_points now returns Vec<f64> directly (no conversion needed!)
        let points = bits_to_points(&decompressed, &req.metadata.origin, req.metadata.resolution);

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

pub fn create_worker_pool(callback_events_tx: Sender<CallbackEvent>) -> Arc<LidarWorkerPool> {
    Arc::new(LidarWorkerPool::new(callback_events_tx, 2))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossbeam_channel::bounded;
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
        let (cb_tx, cb_rx) = bounded::<CallbackEvent>(8);
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
            topic: "rt/utlidar/voxel_map_compressed".to_string(),
            payload: json!({"data": {}}),
            compressed_data: compressed,
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
        let (cb_tx, _cb_rx) = bounded::<CallbackEvent>(8);
        let pool = create_worker_pool(cb_tx);

        let data = vec![0u8; 3];
        let compressed = lz4_flex::block::compress(&data);

        let req = LidarDecodeRequest {
            topic: "rt/utlidar/voxel_map_compressed".to_string(),
            payload: json!({}),
            compressed_data: compressed,
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

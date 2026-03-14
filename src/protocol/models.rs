use bytes::Bytes;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionState {
    Disconnected,
    Connecting,
    Connected,
    Reconnecting,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MessageEnvelope {
    #[serde(rename = "type")]
    pub type_: String,
    #[serde(default)]
    pub topic: String,
    #[serde(default)]
    pub data: Value,
}

impl MessageEnvelope {
    pub fn correlation_key(&self) -> String {
        format!("{} $ {}", self.type_, self.topic)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequestIdentity(pub String);

impl RequestIdentity {
    pub fn extract(payload: &Value) -> Option<Self> {
        Self::extract_key(payload).map(Self)
    }

    pub fn extract_key(payload: &Value) -> Option<String> {
        if let Some(value) = nested_scalar_to_string(payload, &["uuid"]) {
            return Some(value);
        }

        if let Some(value) = nested_scalar_to_string(payload, &["header", "identity", "id"]) {
            return Some(value);
        }

        if let Some(value) = nested_scalar_to_string(payload, &["req_uuid"]) {
            return Some(value);
        }

        let type_ = nested_scalar_to_string(payload, &["type"]).unwrap_or_default();
        let topic = nested_scalar_to_string(payload, &["topic"]).unwrap_or_default();
        if type_.is_empty() && topic.is_empty() {
            return None;
        }

        Some(format!("{type_} $ {topic}"))
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum DcMessage {
    Text(String),
    Binary(Bytes),
}

#[derive(Debug, Clone, PartialEq)]
pub enum CallbackEvent {
    TopicCallback {
        topic: String,
        payload: Value,
    },
    LidarCallback {
        topic: String,
        payload: Value,
        points: Vec<f32>,
    },
    VideoFrame {
        data: Vec<u8>,
        width: u32,
        height: u32,
    },
    AudioFrame {
        data: Vec<i16>,
        sample_rate: u32,
        channels: u16,
    },
    FutureResolve {
        key: String,
        result: Value,
    },
}

fn nested_scalar_to_string(payload: &Value, keys: &[&str]) -> Option<String> {
    let mut current = payload;

    for key in keys {
        current = current.get(*key)?;
    }

    if let Some(value) = current.as_str() {
        return Some(value.to_string());
    }

    if let Some(value) = current.as_i64() {
        return Some(value.to_string());
    }

    if let Some(value) = current.as_u64() {
        return Some(value.to_string());
    }

    if let Some(value) = current.as_f64() {
        return Some(value.to_string());
    }

    if let Some(value) = current.as_bool() {
        return Some(value.to_string());
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn extract_key_prefers_numeric_header_identity_id() {
        let payload = json!({
            "header": {
                "identity": {
                    "id": 123456789
                }
            }
        });

        assert_eq!(
            RequestIdentity::extract_key(&payload),
            Some("123456789".to_string())
        );
    }

    #[test]
    fn extract_key_uses_uuid_then_req_uuid() {
        let with_uuid = json!({ "uuid": "abc-uuid" });
        let with_req_uuid = json!({ "req_uuid": "req-uuid" });

        assert_eq!(
            RequestIdentity::extract_key(&with_uuid),
            Some("abc-uuid".to_string())
        );
        assert_eq!(
            RequestIdentity::extract_key(&with_req_uuid),
            Some("req-uuid".to_string())
        );
    }

    #[test]
    fn extract_key_falls_back_to_type_and_topic() {
        let payload = json!({
            "type": "res",
            "topic": "rt/api/sport/request"
        });

        assert_eq!(
            RequestIdentity::extract_key(&payload),
            Some("res $ rt/api/sport/request".to_string())
        );
    }

    // ── MessageEnvelope ───────────────────────────────────────────────────────

    /// MessageEnvelope serializes and deserializes correctly (round-trip).
    #[test]
    fn message_envelope_serde_round_trip() {
        let env = MessageEnvelope {
            type_: "req".to_string(),
            topic: "rt/api/sport/request".to_string(),
            data: json!({"header": {"identity": {"api_id": 1034}}}),
        };
        let serialized = serde_json::to_string(&env).unwrap();
        let deserialized: MessageEnvelope = serde_json::from_str(&serialized).unwrap();
        assert_eq!(deserialized.type_, "req");
        assert_eq!(deserialized.topic, "rt/api/sport/request");
        assert_eq!(deserialized.data["header"]["identity"]["api_id"], 1034);
    }

    // ── SessionState ──────────────────────────────────────────────────────────

    /// All four SessionState variants exist.
    #[test]
    fn session_state_has_four_variants() {
        let states = [
            SessionState::Disconnected,
            SessionState::Connecting,
            SessionState::Connected,
            SessionState::Reconnecting,
        ];
        assert_eq!(states.len(), 4);
    }

    // ── DcMessage ─────────────────────────────────────────────────────────────

    /// DcMessage::Text and DcMessage::Binary compare correctly.
    #[test]
    fn dc_message_equality() {
        assert_eq!(
            DcMessage::Text("hello".to_string()),
            DcMessage::Text("hello".to_string())
        );
        assert_ne!(
            DcMessage::Text("hello".to_string()),
            DcMessage::Text("world".to_string())
        );
        assert_eq!(
            DcMessage::Binary(vec![1, 2, 3].into()),
            DcMessage::Binary(vec![1, 2, 3].into())
        );
        assert_ne!(
            DcMessage::Binary(vec![1, 2].into()),
            DcMessage::Binary(vec![1, 2, 3].into())
        );
    }

    // ── CallbackEvent ─────────────────────────────────────────────────────

    /// VideoFrame callback event has correct shape with data, width, height.
    #[test]
    fn callback_event_video_frame_has_correct_shape() {
        let frame = CallbackEvent::VideoFrame {
            data: vec![128u8; 1920 * 1080 * 3],
            width: 1920,
            height: 1080,
        };

        match frame {
            CallbackEvent::VideoFrame {
                data,
                width,
                height,
            } => {
                assert_eq!(data.len(), 1920 * 1080 * 3);
                assert_eq!(width, 1920);
                assert_eq!(height, 1080);
            }
            _ => panic!("Expected VideoFrame variant"),
        }
    }

    /// AudioFrame callback event has correct shape with data, sample_rate, channels.
    #[test]
    fn callback_event_audio_frame_has_correct_shape() {
        let frame = CallbackEvent::AudioFrame {
            data: vec![0i16; 960],
            sample_rate: 48000,
            channels: 2,
        };

        match frame {
            CallbackEvent::AudioFrame {
                data,
                sample_rate,
                channels,
            } => {
                assert_eq!(data.len(), 960);
                assert_eq!(sample_rate, 48000);
                assert_eq!(channels, 2);
            }
            _ => panic!("Expected AudioFrame variant"),
        }
    }

    /// LidarCallback event has topic, payload, and points (xyz triplets).
    #[test]
    fn callback_event_lidar_callback_has_correct_shape() {
        let points = vec![1.0, 2.0, 3.0, 4.0, 5.0, 6.0]; // 2 points
        let event = CallbackEvent::LidarCallback {
            topic: "rt/utlidar/voxel_map_compressed".to_string(),
            payload: serde_json::json!({"data": {}}),
            points: points.clone(),
        };

        match event {
            CallbackEvent::LidarCallback {
                topic,
                payload: _,
                points,
            } => {
                assert_eq!(topic, "rt/utlidar/voxel_map_compressed");
                assert_eq!(points.len(), 6);
                assert_eq!(points.len() % 3, 0, "points should be xyz triplets");
            }
            _ => panic!("Expected LidarCallback variant"),
        }
    }
}

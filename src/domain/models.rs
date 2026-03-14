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
    Binary(Vec<u8>),
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
        points: Vec<f64>,
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
    use super::RequestIdentity;
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
}

use crate::protocol::ports::PortResult;
use serde_json::Value;
use webrtc::peer_connection::sdp::session_description::RTCSessionDescription;

pub fn parse_answer_payload(answer_payload: &str) -> PortResult<RTCSessionDescription> {
    if let Ok(value) = serde_json::from_str::<Value>(answer_payload) {
        // Check for rejection
        if value.get("sdp").and_then(Value::as_str) == Some("reject") {
            return Err(
                "Go2 is connected by another WebRTC client. Close your mobile APP and try again."
                    .to_string(),
            );
        }

        // Try to parse as full RTCSessionDescription
        if let Ok(answer) = serde_json::from_value::<RTCSessionDescription>(value.clone()) {
            return Ok(answer);
        }

        // Extract SDP string from JSON
        if let Some(sdp) = value.get("sdp").and_then(Value::as_str) {
            return RTCSessionDescription::answer(sdp.to_string())
                .map_err(|error| error.to_string());
        }
    }

    // Fallback: treat entire payload as SDP string
    RTCSessionDescription::answer(answer_payload.to_string()).map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_answer_detects_reject() {
        let payload = r#"{"type":"answer","sdp":"reject"}"#;
        let result = parse_answer_payload(payload);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("another WebRTC client"));
    }
}

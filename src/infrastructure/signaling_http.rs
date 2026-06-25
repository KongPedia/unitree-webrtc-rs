use crate::infrastructure::security::{
    aes_ecb_decrypt, aes_ecb_encrypt, decrypt_con_notify_data,
    decrypt_con_notify_data_with_aes_128_key, generate_aes_key, rsa_encrypt, rsa_load_public_key,
};
use crate::protocol::ports::{PortResult, SignalingPort};
use base64::engine::general_purpose::STANDARD;
use base64::Engine as _;
use reqwest::Client;
use serde_json::{json, Value};
use std::future::Future;
use std::pin::Pin;

#[derive(Debug, Clone, Default)]
pub struct HttpSignalingClient {
    aes_128_key: Option<String>,
}

impl HttpSignalingClient {
    pub fn new(aes_128_key: Option<String>) -> Self {
        Self {
            aes_128_key: normalize_optional_key(aes_128_key),
        }
    }
}

pub async fn send_sdp_local_old(ip: &str, sdp_json: &str) -> Result<String, String> {
    let url = format!("http://{ip}:8081/offer");
    let client = Client::new();

    let response = client
        .post(url)
        .header("Content-Type", "application/json")
        .body(sdp_json.to_string())
        .send()
        .await
        .map_err(|error| error.to_string())?;

    if !response.status().is_success() {
        return Err(format!(
            "Failed to receive SDP answer from old method: {}",
            response.status()
        ));
    }

    response.text().await.map_err(|error| error.to_string())
}

pub async fn send_sdp_local_new(ip: &str, sdp_json: &str) -> Result<String, String> {
    send_sdp_local_new_with_aes_key(ip, sdp_json, None).await
}

async fn send_sdp_local_new_with_aes_key(
    ip: &str,
    sdp_json: &str,
    aes_128_key: Option<&str>,
) -> Result<String, String> {
    let client = Client::new();
    let notify_url = format!("http://{ip}:9991/con_notify");

    let response = client
        .post(notify_url)
        .send()
        .await
        .map_err(|error| error.to_string())?;

    if !response.status().is_success() {
        return Err(format!(
            "Failed to receive con_notify response: {}",
            response.status()
        ));
    }

    let encoded_payload = response.text().await.map_err(|error| error.to_string())?;
    let decoded_payload = STANDARD
        .decode(encoded_payload)
        .map_err(|error| error.to_string())?;
    let decoded_text = String::from_utf8(decoded_payload).map_err(|error| error.to_string())?;

    let mut decoded_json: Value =
        serde_json::from_str(&decoded_text).map_err(|error| error.to_string())?;
    decoded_json["data1"] = Value::String(decode_con_notify_data1(&decoded_json, aes_128_key)?);

    let data1 = decoded_json
        .get("data1")
        .and_then(Value::as_str)
        .ok_or_else(|| "con_notify data1 field missing".to_string())?;

    if data1.len() < 20 {
        return Err("con_notify data1 is too short".to_string());
    }

    let public_key_pem = &data1[10..data1.len() - 10];
    let path_ending = calc_local_path_ending(data1);

    let aes_key = generate_aes_key();
    let public_key = rsa_load_public_key(public_key_pem)?;

    let body = json!({
        "data1": aes_ecb_encrypt(sdp_json, &aes_key)?,
        "data2": rsa_encrypt(&aes_key, &public_key)?,
    });

    let ingest_url = format!("http://{ip}:9991/con_ing_{path_ending}");
    let response = client
        .post(ingest_url)
        .header("Content-Type", "application/x-www-form-urlencoded")
        .body(body.to_string())
        .send()
        .await
        .map_err(|error| error.to_string())?;

    if !response.status().is_success() {
        return Err(format!(
            "Failed to receive con_ing response: {}",
            response.status()
        ));
    }

    let encrypted_answer = response.text().await.map_err(|error| error.to_string())?;
    aes_ecb_decrypt(&encrypted_answer, &aes_key)
}

pub async fn send_sdp_local(ip: &str, sdp_json: &str) -> Result<String, String> {
    send_sdp_local_with_aes_key(ip, sdp_json, None).await
}

pub async fn send_sdp_local_with_aes_key(
    ip: &str,
    sdp_json: &str,
    aes_128_key: Option<&str>,
) -> Result<String, String> {
    match send_sdp_local_old(ip, sdp_json).await {
        Ok(answer) => Ok(answer),
        Err(legacy_error) => send_sdp_local_new_with_aes_key(ip, sdp_json, aes_128_key)
            .await
            .map_err(|new_error| {
                format!(
                    "legacy offer failed ({legacy_error}); con_notify offer failed ({new_error})"
                )
            }),
    }
}

pub async fn send_sdp_remote(_serial: &str, _sdp_json: &str) -> Result<String, String> {
    Err("Remote signaling is not implemented in phase 1".to_string())
}

impl SignalingPort for HttpSignalingClient {
    fn exchange_sdp<'a>(
        &'a self,
        ip: &'a str,
        offer: &'a str,
    ) -> Pin<Box<dyn Future<Output = PortResult<String>> + Send + 'a>> {
        Box::pin(async move {
            send_sdp_local_with_aes_key(ip, offer, self.aes_128_key.as_deref()).await
        })
    }
}

fn normalize_optional_key(key: Option<String>) -> Option<String> {
    key.map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn decode_con_notify_data1(
    decoded_json: &Value,
    aes_128_key: Option<&str>,
) -> Result<String, String> {
    let data1 = decoded_json
        .get("data1")
        .and_then(Value::as_str)
        .ok_or_else(|| "con_notify data1 field missing".to_string())?;
    let data2 = match decoded_json.get("data2") {
        Some(value) => value
            .as_i64()
            .ok_or_else(|| format!("con_notify data2 must be an integer, got {value}"))?,
        None => 0,
    };

    match data2 {
        0 => Ok(data1.to_string()),
        2 => decrypt_con_notify_data(data1),
        3 => {
            let key = aes_128_key
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .ok_or_else(|| {
                    "con_notify data2=3 requires aes_128_key or UNITREE_AES_128_KEY/GO2_DORA_UNITREE_AES_128_KEY"
                        .to_string()
                })?;
            decrypt_con_notify_data_with_aes_128_key(data1, key)
        }
        mode => Err(format!("unsupported con_notify data2 auth mode: {mode}")),
    }
}

fn calc_local_path_ending(data1: &str) -> String {
    let symbols = ["A", "B", "C", "D", "E", "F", "G", "H", "I", "J"];
    let chars: Vec<char> = data1.chars().collect();

    let last_ten: Vec<char> = chars.into_iter().rev().take(10).collect::<Vec<_>>();
    if last_ten.len() < 10 {
        return String::new();
    }

    let ordered_last_ten: Vec<char> = last_ten.into_iter().rev().collect();
    let mut output = String::new();

    for pair in ordered_last_ten.chunks(2) {
        if pair.len() < 2 {
            continue;
        }

        let second = pair[1].to_string();
        if let Some(index) = symbols.iter().position(|symbol| *symbol == second) {
            output.push_str(&index.to_string());
        }
    }

    output
}

#[cfg(test)]
mod tests {
    use super::{calc_local_path_ending, decode_con_notify_data1};
    use aes_gcm::aead::Aead;
    use aes_gcm::{Aes128Gcm, KeyInit, Nonce};
    use base64::engine::general_purpose::STANDARD;
    use base64::Engine as _;
    use serde_json::json;

    #[test]
    fn path_ending_uses_second_char_index() {
        let input = "prefix123450A1B2C3D4E";
        assert_eq!(calc_local_path_ending(input), "01234");
    }

    #[test]
    fn short_string_returns_empty() {
        assert!(calc_local_path_ending("short").is_empty());
    }

    #[test]
    fn con_notify_data2_plaintext_passes_through() {
        let decoded_json = json!({"data1": "plain", "data2": 0});

        assert_eq!(
            decode_con_notify_data1(&decoded_json, None).unwrap(),
            "plain"
        );
    }

    #[test]
    fn con_notify_missing_data2_defaults_to_plaintext() {
        let decoded_json = json!({"data1": "plain"});

        assert_eq!(
            decode_con_notify_data1(&decoded_json, None).unwrap(),
            "plain"
        );
    }

    #[test]
    fn con_notify_unknown_data2_is_explicit_error() {
        let decoded_json = json!({"data1": "encrypted", "data2": 4});

        let err = decode_con_notify_data1(&decoded_json, None).unwrap_err();

        assert!(err.contains("unsupported con_notify data2 auth mode: 4"));
    }

    #[test]
    fn con_notify_data2_3_requires_per_device_key() {
        let decoded_json = json!({"data1": "encrypted", "data2": 3});

        let err = decode_con_notify_data1(&decoded_json, None).unwrap_err();

        assert!(err.contains("data2=3"));
        assert!(err.contains("aes_128_key"));
    }

    #[test]
    fn con_notify_data2_3_uses_per_device_key() {
        let key_hex = "00112233445566778899aabbccddeeff";
        let key: [u8; 16] = hex::decode(key_hex).unwrap().try_into().unwrap();
        let decoded_json = json!({
            "data1": encode_data2_3_payload(&key, b"v3-public-key-payload"),
            "data2": 3,
        });

        assert_eq!(
            decode_con_notify_data1(&decoded_json, Some(key_hex)).unwrap(),
            "v3-public-key-payload"
        );
    }

    fn encode_data2_3_payload(key: &[u8; 16], plain: &[u8]) -> String {
        let cipher = Aes128Gcm::new_from_slice(key).unwrap();
        let nonce = [5_u8; 12];
        let ciphertext_with_tag = cipher
            .encrypt(Nonce::from_slice(&nonce), plain.as_ref())
            .unwrap();

        let split_index = ciphertext_with_tag.len() - 16;
        let mut packed = Vec::new();
        packed.extend_from_slice(&ciphertext_with_tag[..split_index]);
        packed.extend_from_slice(&nonce);
        packed.extend_from_slice(&ciphertext_with_tag[split_index..]);

        STANDARD.encode(packed)
    }
}

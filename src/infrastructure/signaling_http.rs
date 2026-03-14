use crate::domain::ports::{PortResult, SignalingPort};
use crate::infrastructure::security::{
    aes_ecb_decrypt, aes_ecb_encrypt, decrypt_con_notify_data, generate_aes_key, rsa_encrypt,
    rsa_load_public_key,
};
use base64::engine::general_purpose::STANDARD;
use base64::Engine as _;
use reqwest::Client;
use serde_json::{json, Value};
use std::future::Future;
use std::pin::Pin;

#[derive(Debug, Clone, Default)]
pub struct HttpSignalingClient;

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
    let data2 = decoded_json
        .get("data2")
        .and_then(Value::as_i64)
        .unwrap_or_default();

    if data2 == 2 {
        let encrypted = decoded_json
            .get("data1")
            .and_then(Value::as_str)
            .ok_or_else(|| "con_notify data1 field missing".to_string())?;
        let decrypted = decrypt_con_notify_data(encrypted)?;
        decoded_json["data1"] = Value::String(decrypted);
    }

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
    match send_sdp_local_old(ip, sdp_json).await {
        Ok(answer) => Ok(answer),
        Err(_) => send_sdp_local_new(ip, sdp_json).await,
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
        Box::pin(async move { send_sdp_local(ip, offer).await })
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
    use super::calc_local_path_ending;

    #[test]
    fn path_ending_uses_second_char_index() {
        let input = "prefix123450A1B2C3D4E";
        assert_eq!(calc_local_path_ending(input), "01234");
    }

    #[test]
    fn short_string_returns_empty() {
        assert!(calc_local_path_ending("short").is_empty());
    }
}

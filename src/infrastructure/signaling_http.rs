use crate::infrastructure::security::{
    aes_ecb_decrypt, aes_ecb_encrypt, decrypt_con_notify_data,
    decrypt_con_notify_data_with_aes_128_key, generate_aes_key, rsa_encrypt, rsa_load_public_key,
};
use crate::protocol::ports::{PortResult, SignalingPort};
use base64::engine::general_purpose::STANDARD;
use base64::Engine as _;
use reqwest::{Client, Url};
use serde_json::{json, Value};
use std::future::Future;
use std::net::{IpAddr, Ipv6Addr};
use std::pin::Pin;
use std::time::Duration;

const SIGNALING_BODY_LIMIT: usize = 1024 * 1024;
const SIGNALING_REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Clone)]
pub struct HttpSignalingClient {
    aes_128_key: Option<String>,
}

impl std::fmt::Debug for HttpSignalingClient {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("HttpSignalingClient")
            .field(
                "aes_128_key",
                &self.aes_128_key.as_ref().map(|_| "[REDACTED]"),
            )
            .finish()
    }
}

impl HttpSignalingClient {
    pub fn new(aes_128_key: Option<String>) -> Self {
        crate::infrastructure::ensure_crypto_provider();
        Self {
            aes_128_key: normalize_optional_key(aes_128_key),
        }
    }
}

impl Default for HttpSignalingClient {
    fn default() -> Self {
        Self::new(None)
    }
}

fn signaling_client() -> Result<Client, String> {
    Client::builder()
        .timeout(SIGNALING_REQUEST_TIMEOUT)
        .redirect(reqwest::redirect::Policy::none())
        .no_proxy()
        .build()
        .map_err(|_| "Signaling HTTP client initialization failed".into())
}

fn signaling_url(host: &str, port: u16, path: &str) -> Result<Url, String> {
    let invalid = || "Invalid signaling host: expected only an IP address or hostname".to_string();
    if host.is_empty() || host.len() > 254 {
        return Err(invalid());
    }
    let ip = if let Some(inner) = host.strip_prefix('[') {
        Some(IpAddr::V6(
            inner
                .strip_suffix(']')
                .ok_or_else(invalid)?
                .parse::<Ipv6Addr>()
                .map_err(|_| invalid())?,
        ))
    } else if let Ok(ip) = host.parse::<IpAddr>() {
        Some(ip)
    } else {
        let name = host.strip_suffix('.').unwrap_or(host);
        if name.is_empty()
            || name.len() > 253
            || name
                .bytes()
                .all(|byte| byte.is_ascii_digit() || byte == b'.')
            || !name.split('.').all(|label| {
                !label.is_empty()
                    && label.len() <= 63
                    && label.as_bytes()[0].is_ascii_alphanumeric()
                    && label.as_bytes()[label.len() - 1].is_ascii_alphanumeric()
                    && label
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
            })
        {
            return Err(invalid());
        }
        None
    };

    let mut url = Url::parse("http://localhost/").map_err(|_| invalid())?;
    if let Some(ip) = ip {
        url.set_ip_host(ip).map_err(|_| invalid())?;
    } else {
        // set_host alone accepts/truncates authority suffixes; validate the entire input first.
        url.set_host(Some(host)).map_err(|_| invalid())?;
        if !url
            .host_str()
            .is_some_and(|parsed| parsed.eq_ignore_ascii_case(host))
        {
            return Err(invalid());
        }
    }
    url.set_port(Some(port)).map_err(|_| invalid())?;
    url.set_path(path);
    Ok(url)
}

fn signaling_error(prefix: &str, error: &reqwest::Error) -> String {
    let reason = if error.is_timeout() {
        "timeout"
    } else if error.is_connect() {
        "connect"
    } else if error.is_body() {
        "body"
    } else if error.is_decode() {
        "decode"
    } else {
        "request"
    };
    format!("{prefix}: {reason}")
}

async fn read_response(mut response: reqwest::Response) -> Result<String, String> {
    if response
        .content_length()
        .is_some_and(|size| size > SIGNALING_BODY_LIMIT as u64)
    {
        return Err("Signaling response body exceeds 1 MiB".into());
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|error| signaling_error("Signaling response read failed", &error))?
    {
        if chunk.len() > SIGNALING_BODY_LIMIT - bytes.len() {
            return Err("Signaling response body exceeds 1 MiB".into());
        }
        bytes.extend_from_slice(&chunk);
    }
    String::from_utf8(bytes).map_err(|_| "Signaling response is not UTF-8".into())
}

pub async fn send_sdp_local_old(ip: &str, sdp_json: &str) -> Result<String, String> {
    let url = signaling_url(ip, 8081, "/offer")?;
    crate::infrastructure::ensure_crypto_provider();
    let client = signaling_client()?;

    let response = client
        .post(url)
        .header("Content-Type", "application/json")
        .body(sdp_json.to_string())
        .send()
        .await
        .map_err(|error| signaling_error("Signaling request failed", &error))?;

    if !response.status().is_success() {
        return Err(format!(
            "Failed to receive SDP answer from old method: {}",
            response.status()
        ));
    }

    read_response(response).await
}

pub async fn send_sdp_local_new(ip: &str, sdp_json: &str) -> Result<String, String> {
    send_sdp_local_new_with_aes_key(ip, sdp_json, None).await
}

async fn send_sdp_local_new_with_aes_key(
    ip: &str,
    sdp_json: &str,
    aes_128_key: Option<&str>,
) -> Result<String, String> {
    let notify_url = signaling_url(ip, 9991, "/con_notify")?;
    crate::infrastructure::ensure_crypto_provider();
    let client = signaling_client()?;

    let response = client
        .post(notify_url)
        .send()
        .await
        .map_err(|error| signaling_error("Signaling request failed", &error))?;

    if !response.status().is_success() {
        return Err(format!(
            "Failed to receive con_notify response: {}",
            response.status()
        ));
    }

    let encoded_payload = read_response(response).await?;
    let decoded_payload = STANDARD
        .decode(encoded_payload)
        .map_err(|_| "Invalid con_notify base64".to_string())?;
    let decoded_text =
        String::from_utf8(decoded_payload).map_err(|_| "Invalid con_notify UTF-8".to_string())?;

    let mut decoded_json: Value =
        serde_json::from_str(&decoded_text).map_err(|_| "Invalid con_notify JSON".to_string())?;
    decoded_json["data1"] = Value::String(decode_con_notify_data1(&decoded_json, aes_128_key)?);

    let data1 = decoded_json
        .get("data1")
        .and_then(Value::as_str)
        .ok_or_else(|| "con_notify data1 field missing".to_string())?;

    let public_key_pem = con_notify_public_key(data1)?;
    let path_ending = calc_local_path_ending(data1);

    let aes_key = generate_aes_key();
    let public_key = rsa_load_public_key(public_key_pem)?;

    let body = json!({
        "data1": aes_ecb_encrypt(sdp_json, &aes_key)?,
        "data2": rsa_encrypt(&aes_key, &public_key)?,
    });

    let ingest_url = signaling_url(ip, 9991, &format!("/con_ing_{path_ending}"))?;
    let response = client
        .post(ingest_url)
        .header("Content-Type", "application/x-www-form-urlencoded")
        .body(body.to_string())
        .send()
        .await
        .map_err(|error| signaling_error("Signaling request failed", &error))?;

    if !response.status().is_success() {
        return Err(format!(
            "Failed to receive con_ing response: {}",
            response.status()
        ));
    }

    let encrypted_answer = read_response(response).await?;
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
            .ok_or_else(|| "con_notify data2 must be an integer".to_string())?,
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
                    "con_notify data2=3 requires aes_128_key or GO2_DORA_UNITREE_AES_128_KEY"
                        .to_string()
                })?;
            decrypt_con_notify_data_with_aes_128_key(data1, key)
        }
        mode => Err(format!("unsupported con_notify data2 auth mode: {mode}")),
    }
}

fn con_notify_public_key(data1: &str) -> Result<&str, String> {
    if data1.len() < 20 {
        return Err("con_notify data1 is too short".to_string());
    }
    data1
        .get(10..data1.len() - 10)
        .ok_or_else(|| "con_notify data1 wrapper has invalid UTF-8 boundaries".to_string())
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
    use super::{
        calc_local_path_ending, con_notify_public_key, decode_con_notify_data1, HttpSignalingClient,
    };
    use aes_gcm::aead::Aead;
    use aes_gcm::{Aes128Gcm, KeyInit, Nonce};
    use base64::engine::general_purpose::STANDARD;
    use base64::Engine as _;
    use serde_json::json;

    #[test]
    fn boundary_signaling_builds_only_structured_local_endpoints() {
        for (host, authority) in [
            ("192.168.123.161", "192.168.123.161"),
            ("go2-01.local", "go2-01.local"),
            ("go2-01.local.", "go2-01.local."),
            ("::1", "[::1]"),
            ("[2001:db8::1]", "[2001:db8::1]"),
        ] {
            let url = super::signaling_url(host, 9991, "/con_notify").unwrap();
            assert_eq!(url.as_str(), format!("http://{authority}:9991/con_notify"));
            assert!(url.username().is_empty());
            assert!(url.password().is_none());
            assert!(url.query().is_none());
            assert!(url.fragment().is_none());
        }
    }

    #[test]
    fn boundary_signaling_rejects_ambiguous_numeric_and_invalid_dns_hosts() {
        for host in [
            "",
            ".",
            "127.1",
            "0x7f.0.0.1",
            "0177.0.0.1",
            "2130706433",
            "go2..local",
            "-go2.local",
            "go2-.local",
            "[::1",
            "::1]",
            "[::1%25zone]",
            "[::1]:123",
        ] {
            let error = super::signaling_url(host, 8081, "/offer").err();
            assert_eq!(
                error.as_deref(),
                Some("Invalid signaling host: expected only an IP address or hostname"),
                "noncanonical or malformed host admitted"
            );
        }
        for host in ["x".repeat(64), "x".repeat(255)] {
            assert!(super::signaling_url(&host, 8081, "/offer").is_err());
        }
    }

    #[tokio::test]
    async fn boundary_signaling_ignores_environment_proxies() {
        const TEST: &str =
            "infrastructure::signaling_http::tests::boundary_signaling_ignores_environment_proxies";
        const FIXTURE_ENV: &str = "UNITREE_SIGNALING_PROXY_FIXTURE";
        if std::env::var(FIXTURE_ENV).ok().as_deref() != Some("1") {
            // Keep process-global proxy settings out of unrelated parallel tests.
            let mut command = std::process::Command::new(std::env::current_exe().unwrap());
            command
                .args(["--exact", TEST, "--nocapture"])
                .env(FIXTURE_ENV, "1");
            for name in [
                "HTTP_PROXY",
                "http_proxy",
                "HTTPS_PROXY",
                "https_proxy",
                "ALL_PROXY",
                "all_proxy",
            ] {
                command.env(name, "http://127.0.0.1:9");
            }
            command.env("NO_PROXY", "").env("no_proxy", "");
            let output = command.output().unwrap();
            assert!(
                output.status.success(),
                "proxy fixture failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            assert!(String::from_utf8_lossy(&output.stdout).contains("1 passed;"));
            return;
        }
        let response = response_fixture(false, 5).await;
        assert_eq!(super::read_response(response).await.unwrap(), "xxxxx");
    }

    #[tokio::test]
    async fn boundary_signaling_rejects_non_host_input_before_requesting() {
        for host in [
            "127.0.0.1:54321",
            "sentinel-private-key@127.0.0.1",
            "127.0.0.1/sentinel-private-key",
            "127.0.0.1?sentinel-private-key",
            "127.0.0.1#sentinel-private-key",
            "127.0.0.1\\sentinel-private-key",
            "127.0.0.1%2fsentinel-private-key",
            "http://127.0.0.1",
            " 127.0.0.1",
            "127.0.0.1\t",
            "[::1]:54321",
            "[::1]suffix",
        ] {
            assert_eq!(
                super::send_sdp_local_old(host, "{}").await.unwrap_err(),
                "Invalid signaling host: expected only an IP address or hostname",
                "malformed host was not rejected at admission"
            );
            assert_eq!(
                super::send_sdp_local_new(host, "{}").await.unwrap_err(),
                "Invalid signaling host: expected only an IP address or hostname"
            );
        }
    }

    #[tokio::test]
    async fn boundary_signaling_does_not_follow_peer_redirects() {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            let mut request = [0; 4096];
            assert!(socket.read(&mut request).unwrap() > 0);
            write!(socket, "HTTP/1.1 307 Temporary Redirect\r\nLocation: http://127.0.0.1:{}/sentinel-private-key\r\nContent-Length: 0\r\nConnection: close\r\n\r\n", address.port()).unwrap();
            // Dropping the listener makes a mistakenly followed redirect fail immediately.
        });
        crate::infrastructure::ensure_crypto_provider();
        let response = super::signaling_client()
            .unwrap()
            .post(format!("http://{address}/offer"))
            .body("sentinel-private-sdp")
            .send()
            .await;
        server.join().unwrap();
        assert!(
            response.is_ok_and(|reply| reply.status() == reqwest::StatusCode::TEMPORARY_REDIRECT),
            "signaling client followed the redirect instead of returning its status"
        );
    }

    async fn response_fixture(chunked: bool, size: usize) -> reqwest::Response {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            let mut request = [0; 4096];
            let _ = socket.read(&mut request);
            let mut reply = if chunked {
                format!("HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n{size:x}\r\n").into_bytes()
            } else {
                format!("HTTP/1.1 200 OK\r\nContent-Length: {size}\r\nConnection: close\r\n\r\n")
                    .into_bytes()
            };
            reply.resize(reply.len() + size, b'x');
            if chunked {
                reply.extend_from_slice(b"\r\n0\r\n\r\n");
            }
            let _ = socket.write_all(&reply);
        });
        crate::infrastructure::ensure_crypto_provider();
        super::signaling_client()
            .unwrap()
            .get(format!("http://{address}"))
            .send()
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn signaling_errors_keep_safe_classes_without_url_secrets() {
        crate::infrastructure::ensure_crypto_provider();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        drop(listener);
        let client = super::signaling_client().unwrap();
        let error = client
            .get(format!(
                "http://sentinel-user:sentinel-secret@{address}/private-path"
            ))
            .send()
            .await
            .unwrap_err();
        assert_eq!(
            super::signaling_error("Signaling request failed", &error),
            "Signaling request failed: connect"
        );
        let error = client
            .get("http://sentinel-user:sentinel-secret@[invalid/private-path")
            .send()
            .await
            .unwrap_err();
        assert_eq!(
            super::signaling_error("Signaling request failed", &error),
            "Signaling request failed: request"
        );
    }

    #[tokio::test]
    async fn signaling_request_deadline_covers_stalled_response_body() {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            let mut request = [0; 4096];
            let _ = socket.read(&mut request);
            socket
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 1\r\n\r\n")
                .unwrap();
            std::thread::sleep(std::time::Duration::from_secs(12));
        });
        crate::infrastructure::ensure_crypto_provider();
        let response = super::signaling_client()
            .unwrap()
            .get(format!("http://{address}"))
            .send()
            .await
            .unwrap();
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(11),
            super::read_response(response),
        )
        .await
        .unwrap();
        assert_eq!(
            result.unwrap_err(),
            "Signaling response read failed: timeout"
        );
    }

    #[tokio::test]
    async fn signaling_response_bounds_length_and_chunked_bodies() {
        for chunked in [false, true] {
            let response = response_fixture(chunked, 1024 * 1024 + 1).await;
            assert!(
                super::read_response(response).await.is_err(),
                "oversized body accepted (chunked={chunked})"
            );
        }
    }

    #[tokio::test]
    async fn signaling_response_accepts_body_at_limit() {
        for chunked in [false, true] {
            let response = response_fixture(chunked, 1024 * 1024).await;
            assert_eq!(
                super::read_response(response).await.unwrap().len(),
                1024 * 1024
            );
        }
    }

    #[test]
    fn signaling_debug_redacts_device_key() {
        let client = HttpSignalingClient {
            aes_128_key: Some("sentinel-private-key".into()),
        };
        for debug in [
            format!("{client:?}"),
            format!("{client:#?}"),
            format!("{:?}", client.clone()),
        ] {
            assert!(debug.contains("[REDACTED]"));
            assert!(!debug.contains("sentinel-private-key"));
        }
        let absent = HttpSignalingClient { aes_128_key: None };
        assert!(format!("{absent:?}").contains("None"));
    }

    #[test]
    fn malformed_auth_mode_does_not_echo_peer_payload() {
        for mode in [
            json!("sentinel-private-key"),
            json!({"secret": "sentinel-private-key"}),
            json!(["sentinel-private-key"]),
            json!(null),
            json!(0.5),
        ] {
            let input = json!({"data1": "sentinel-private-key", "data2": mode});
            assert_eq!(
                decode_con_notify_data1(&input, None).unwrap_err(),
                "con_notify data2 must be an integer"
            );
        }
    }

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
    fn public_key_wrapping_rejects_invalid_boundaries_without_panicking() {
        assert_eq!(
            con_notify_public_key("0123456789PEM0123456789").unwrap(),
            "PEM"
        );
        for input in ["short", "aaaaaaaaa가aaaaaaaaaaaa", "aaaaaaaaaa가aaaaaaaaa"] {
            assert!(con_notify_public_key(input).is_err());
        }
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

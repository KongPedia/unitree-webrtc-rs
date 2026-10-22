use crossbeam_channel::bounded;
use std::process::Command;
use std::sync::Arc;
use unitree_webrtc_rs::infrastructure::rtc_engine::RtcEngine;
use unitree_webrtc_rs::infrastructure::signaling_http::HttpSignalingClient;
use unitree_webrtc_rs::protocol::events::event_channel;

#[test]
fn native_constructors_initialize_crypto_without_python() {
    const CASE_ENV: &str = "UNITREE_CRYPTO_INIT_CASE";
    let Ok(case) = std::env::var(CASE_ENV) else {
        // The provider is process-global; each case needs a pristine process.
        for case in [
            "http",
            "default",
            "rtc",
            "host-http",
            "host-default",
            "host-rtc",
        ] {
            let output = Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "native_constructors_initialize_crypto_without_python",
                    "--nocapture",
                ])
                .env(CASE_ENV, case)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{case}: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
        return;
    };

    assert!(rustls::crypto::CryptoProvider::get_default().is_none());
    let host_provider = case.starts_with("host-").then(|| {
        rustls::crypto::ring::default_provider()
            .install_default()
            .unwrap();
        Arc::clone(rustls::crypto::CryptoProvider::get_default().unwrap())
    });
    match case.trim_start_matches("host-") {
        "rtc" => {
            let (incoming_tx, _) = bounded(1);
            let (callback_tx, _) = event_channel(1).unwrap();
            let _ = RtcEngine::new(incoming_tx, callback_tx);
        }
        "http" => {
            let _ = HttpSignalingClient::new(None);
        }
        "default" => {
            let _ = HttpSignalingClient::default();
        }
        _ => panic!("unknown initialization case"),
    }
    let provider = rustls::crypto::CryptoProvider::get_default()
        .expect("native constructor must install crypto provider");
    if let Some(host_provider) = host_provider {
        assert!(
            Arc::ptr_eq(provider, &host_provider),
            "constructor replaced the host provider"
        );
    }
    // This implicit-provider API panics on ambiguous provider features without initialization.
    let _ = rustls::client::WebPkiServerVerifier::builder(Arc::new(rustls::RootCertStore::empty()));
}

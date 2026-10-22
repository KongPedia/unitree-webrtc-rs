pub mod gstreamer_util;
pub mod rtc_engine;
pub mod security;
pub mod signaling_http;

/// Select the native TLS default without replacing a provider installed by the host.
pub fn ensure_crypto_provider() {
    if rustls::crypto::CryptoProvider::get_default().is_none() {
        // Another constructor may win this process-global initialization race.
        let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
    }
}

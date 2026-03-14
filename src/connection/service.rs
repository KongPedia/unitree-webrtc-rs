use crate::protocol::constants::WebRTCConnectionMethod;
use crate::protocol::ports::{RtcEnginePort, SignalingPort};
use serde_json::json;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tracing::{info, warn};

pub struct ConnectionService<S, E>
where
    S: SignalingPort,
    E: RtcEnginePort,
{
    signaling: Arc<S>,
    engine: Arc<E>,
    connection_method: WebRTCConnectionMethod,
    ip: Option<String>,
    is_connected: Arc<AtomicBool>,
    intentional_disconnect: Arc<AtomicBool>,
    reconnecting: Arc<AtomicBool>,
}

impl<S, E> ConnectionService<S, E>
where
    S: SignalingPort,
    E: RtcEnginePort,
{
    pub fn new(
        signaling: Arc<S>,
        engine: Arc<E>,
        connection_method: WebRTCConnectionMethod,
        ip: Option<String>,
    ) -> Self {
        Self {
            signaling,
            engine,
            connection_method,
            ip,
            is_connected: Arc::new(AtomicBool::new(false)),
            intentional_disconnect: Arc::new(AtomicBool::new(false)),
            reconnecting: Arc::new(AtomicBool::new(false)),
        }
    }

    pub async fn connect(&self) -> Result<(), String> {
        self.intentional_disconnect.store(false, Ordering::Relaxed);
        info!(
            event = "webrtc_connect_start",
            connection_method = ?self.connection_method,
            "Starting WebRTC connection"
        );

        let target_ip = match self.connection_method {
            WebRTCConnectionMethod::LocalSTA => self
                .ip
                .clone()
                .ok_or_else(|| "LocalSTA connection requires ip".to_string())?,
            WebRTCConnectionMethod::LocalAP => "192.168.12.1".to_string(),
            WebRTCConnectionMethod::Remote => {
                return Err("Remote connection is not implemented in phase 1".to_string())
            }
        };

        let offer_sdp = self.engine.prepare_offer().await?;
        let offer_payload = json!({
            "id": if self.connection_method == WebRTCConnectionMethod::LocalSTA {
                "STA_localNetwork"
            } else {
                ""
            },
            "sdp": offer_sdp,
            "type": "offer",
            "token": "",
        });

        let answer = self
            .signaling
            .exchange_sdp(&target_ip, &offer_payload.to_string())
            .await?;
        self.engine.apply_answer(&answer).await?;

        self.is_connected.store(true, Ordering::Relaxed);
        info!(event = "webrtc_connect_success", target_ip = %target_ip, "WebRTC connected");
        Ok(())
    }

    pub async fn disconnect(&self) {
        self.intentional_disconnect.store(true, Ordering::Relaxed);
        self.engine.close().await;
        self.is_connected.store(false, Ordering::Relaxed);
        self.reconnecting.store(false, Ordering::Relaxed);
        info!(event = "webrtc_disconnect", "WebRTC disconnected");
    }

    pub async fn reconnect(&self) -> Result<(), String> {
        self.engine.close().await;
        self.is_connected.store(false, Ordering::Relaxed);
        self.connect().await?;
        info!(event = "webrtc_reconnect_success", "WebRTC reconnected");
        Ok(())
    }

    pub async fn auto_reconnect(&self, max_retries: u32) -> bool {
        if self.intentional_disconnect.load(Ordering::Relaxed) {
            return false;
        }

        self.reconnecting.store(true, Ordering::Relaxed);

        for attempt in 0..max_retries {
            if self.intentional_disconnect.load(Ordering::Relaxed) {
                self.reconnecting.store(false, Ordering::Relaxed);
                return false;
            }

            if self.reconnect().await.is_ok() {
                self.reconnecting.store(false, Ordering::Relaxed);
                return true;
            }

            let millis = 200_u64.saturating_mul(1_u64 << attempt.min(8));
            warn!(
                event = "webrtc_reconnect_retry",
                attempt = attempt + 1,
                max_retries,
                backoff_ms = millis,
                "Reconnect attempt failed; waiting before retry"
            );
            tokio::time::sleep(Duration::from_millis(millis)).await;
        }

        self.reconnecting.store(false, Ordering::Relaxed);
        false
    }

    pub fn is_connected(&self) -> bool {
        self.is_connected.load(Ordering::Relaxed)
    }

    pub fn intentional_disconnect(&self) -> bool {
        self.intentional_disconnect.load(Ordering::Relaxed)
    }
}

use crate::domain::ports::{RtcEnginePort, SignalingPort};
use crate::interface::constants::WebRTCConnectionMethod;
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::ports::{PortResult, RtcEnginePort, SignalingPort};
    use std::future::Future;
    use std::pin::Pin;
    use std::sync::Mutex;

    #[derive(Default)]
    struct MockSignaling {
        calls: Arc<Mutex<Vec<(String, String)>>>,
        fail_count: Arc<Mutex<u32>>,
    }

    impl MockSignaling {
        fn with_fail_count(fail_count: u32) -> Self {
            Self {
                calls: Arc::new(Mutex::new(Vec::new())),
                fail_count: Arc::new(Mutex::new(fail_count)),
            }
        }
    }

    impl SignalingPort for MockSignaling {
        fn exchange_sdp<'a>(
            &'a self,
            ip: &'a str,
            offer: &'a str,
        ) -> Pin<Box<dyn Future<Output = PortResult<String>> + Send + 'a>> {
            Box::pin(async move {
                self.calls
                    .lock()
                    .unwrap()
                    .push((ip.to_string(), offer.to_string()));

                let mut remain = self.fail_count.lock().unwrap();
                if *remain > 0 {
                    *remain -= 1;
                    return Err("signaling failed".to_string());
                }

                Ok("answer-sdp".to_string())
            })
        }
    }

    #[derive(Default)]
    struct MockEngine {
        closed_count: Arc<Mutex<u32>>,
        answer_history: Arc<Mutex<Vec<String>>>,
        fail_prepare_count: Arc<Mutex<u32>>,
    }

    impl MockEngine {
        fn with_prepare_fail_count(fail_count: u32) -> Self {
            Self {
                closed_count: Arc::new(Mutex::new(0)),
                answer_history: Arc::new(Mutex::new(Vec::new())),
                fail_prepare_count: Arc::new(Mutex::new(fail_count)),
            }
        }
    }

    impl RtcEnginePort for MockEngine {
        fn prepare_offer<'a>(
            &'a self,
        ) -> Pin<Box<dyn Future<Output = PortResult<String>> + Send + 'a>> {
            Box::pin(async move {
                let mut remain = self.fail_prepare_count.lock().unwrap();
                if *remain > 0 {
                    *remain -= 1;
                    return Err("prepare failed".to_string());
                }
                Ok("offer-sdp".to_string())
            })
        }

        fn apply_answer<'a>(
            &'a self,
            answer_sdp: &'a str,
        ) -> Pin<Box<dyn Future<Output = PortResult<()>> + Send + 'a>> {
            Box::pin(async move {
                self.answer_history
                    .lock()
                    .unwrap()
                    .push(answer_sdp.to_string());
                Ok(())
            })
        }

        fn close<'a>(&'a self) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>> {
            Box::pin(async move {
                let mut count = self.closed_count.lock().unwrap();
                *count += 1;
            })
        }
    }

    #[tokio::test]
    async fn connect_sets_connected_state() {
        let signaling = Arc::new(MockSignaling::default());
        let engine = Arc::new(MockEngine::default());
        let service = ConnectionService::new(
            signaling.clone(),
            engine.clone(),
            WebRTCConnectionMethod::LocalSTA,
            Some("10.0.0.1".to_string()),
        );

        service.connect().await.unwrap();

        assert!(service.is_connected());
        assert_eq!(signaling.calls.lock().unwrap().len(), 1);
        assert_eq!(engine.answer_history.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn disconnect_sets_intentional_state() {
        let signaling = Arc::new(MockSignaling::default());
        let engine = Arc::new(MockEngine::default());
        let service = ConnectionService::new(
            signaling,
            engine.clone(),
            WebRTCConnectionMethod::LocalSTA,
            Some("10.0.0.1".to_string()),
        );

        service.disconnect().await;

        assert!(service.intentional_disconnect());
        assert_eq!(*engine.closed_count.lock().unwrap(), 1);
    }

    #[tokio::test]
    async fn auto_reconnect_succeeds_after_retry() {
        let signaling = Arc::new(MockSignaling::default());
        let engine = Arc::new(MockEngine::with_prepare_fail_count(1));
        let service = ConnectionService::new(
            signaling,
            engine,
            WebRTCConnectionMethod::LocalSTA,
            Some("10.0.0.1".to_string()),
        );

        let result = service.auto_reconnect(3).await;

        assert!(result);
    }

    #[tokio::test]
    async fn auto_reconnect_stops_when_intentional_disconnect() {
        let signaling = Arc::new(MockSignaling::with_fail_count(5));
        let engine = Arc::new(MockEngine::default());
        let service = ConnectionService::new(
            signaling,
            engine,
            WebRTCConnectionMethod::LocalSTA,
            Some("10.0.0.1".to_string()),
        );

        service.disconnect().await;

        let result = service.auto_reconnect(3).await;

        assert!(!result);
    }
}

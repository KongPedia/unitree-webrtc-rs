use crate::protocol::constants::WebRTCConnectionMethod;
use crate::protocol::ports::{RtcEnginePort, SignalingPort};
use serde_json::json;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{watch, Mutex, OwnedMutexGuard};
use tracing::{info, warn};

pub(crate) struct ConnectionAttempt<E: RtcEnginePort + 'static> {
    engine: Arc<E>,
    connected: Arc<AtomicBool>,
    gate: Option<OwnedMutexGuard<()>>,
    runtime: tokio::runtime::Handle,
    cancellation: watch::Receiver<u64>,
    intentional_disconnect: Arc<AtomicBool>,
}

impl<E: RtcEnginePort + 'static> ConnectionAttempt<E> {
    pub(crate) async fn run<T>(
        &mut self,
        operation: impl std::future::Future<Output = Result<T, String>>,
    ) -> Result<T, String> {
        {
            let generation = self.cancellation.borrow();
            if self.cancellation.has_changed().unwrap_or(true) {
                return Err("Connection attempt cancelled by disconnect".into());
            }
            // Synchronize the flag reset with request_disconnect's generation update.
            self.intentional_disconnect.store(false, Ordering::Relaxed);
            drop(generation);
        }
        tokio::select! {
            biased;
            _ = self.cancellation.changed() => Err("Connection attempt cancelled by disconnect".into()),
            result = operation => result,
        }
    }

    pub(crate) fn finish(mut self) {
        self.gate.take();
    }
}

impl<E: RtcEnginePort + 'static> Drop for ConnectionAttempt<E> {
    fn drop(&mut self) {
        let Some(gate) = self.gate.take() else {
            return;
        };
        self.connected.store(false, Ordering::Relaxed);
        let engine = Arc::clone(&self.engine);
        // Retain the gate until cleanup completes; never close a subsequent attempt.
        self.runtime.spawn(async move {
            let _gate = gate;
            engine.close().await;
        });
    }
}

/// Low-level signaling/transport orchestration. `is_connected` does not attest peer validation.
/// Use `UnitreeSession` for the language-neutral validated-session contract.
pub struct ConnectionService<S, E>
where
    S: SignalingPort,
    E: RtcEnginePort + 'static,
{
    signaling: Arc<S>,
    engine: Arc<E>,
    connection_method: WebRTCConnectionMethod,
    ip: Option<String>,
    is_connected: Arc<AtomicBool>,
    intentional_disconnect: Arc<AtomicBool>,
    cancellation: watch::Sender<u64>,
    session_gate: Arc<Mutex<()>>,
}

impl<S, E> ConnectionService<S, E>
where
    S: SignalingPort,
    E: RtcEnginePort + 'static,
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
            cancellation: watch::channel(0).0,
            session_gate: Arc::new(Mutex::new(())),
        }
    }

    pub(crate) async fn lock_session(&self) -> OwnedMutexGuard<()> {
        Arc::clone(&self.session_gate).lock_owned().await
    }

    pub(crate) async fn begin_attempt(&self) -> ConnectionAttempt<E> {
        // Capture before queueing: disconnect must also invalidate queued attempts.
        let cancellation = self.cancellation.subscribe();
        let gate = self.lock_session().await;
        ConnectionAttempt {
            engine: Arc::clone(&self.engine),
            connected: Arc::clone(&self.is_connected),
            gate: Some(gate),
            runtime: tokio::runtime::Handle::current(),
            cancellation,
            intentional_disconnect: Arc::clone(&self.intentional_disconnect),
        }
    }

    pub async fn connect(&self) -> Result<(), String> {
        let mut attempt = self.begin_attempt().await;
        let result = attempt.run(self.connect_in_attempt(false)).await;
        if result.is_err() {
            self.rollback_connection().await;
        }
        attempt.finish();
        result
    }

    pub(crate) async fn connect_in_attempt(&self, reconnect: bool) -> Result<(), String> {
        self.is_connected.store(false, Ordering::Relaxed);
        if reconnect {
            self.rollback_connection().await;
        }
        match tokio::time::timeout(Duration::from_secs(15), self.connect_inner()).await {
            Ok(result) => result,
            Err(_) => {
                let diagnostic = self.engine.timeout_diagnostic();
                warn!(
                    event = "webrtc_connect_timeout",
                    detail = diagnostic.as_deref().unwrap_or("unavailable"),
                    "Connection attempt timed out"
                );
                Err(match diagnostic {
                    Some(detail) => format!("Connection attempt timed out: {detail}"),
                    None => "Connection attempt timed out".to_string(),
                })
            }
        }
    }

    async fn connect_inner(&self) -> Result<(), String> {
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
        info!(event = "webrtc_connect_success", connection_method = ?self.connection_method, "WebRTC connected");
        Ok(())
    }

    pub(crate) fn request_disconnect(&self) {
        self.cancellation.send_modify(|generation| {
            self.intentional_disconnect.store(true, Ordering::Relaxed);
            *generation = generation.wrapping_add(1);
        });
    }

    pub async fn disconnect(&self) {
        self.request_disconnect();
        let attempt = self.begin_attempt().await;
        self.rollback_connection().await;
        attempt.finish();
        info!(event = "webrtc_disconnect", "WebRTC disconnected");
    }

    pub(crate) async fn rollback_connection(&self) {
        self.is_connected.store(false, Ordering::Relaxed);
        self.engine.close().await;
    }

    pub async fn reconnect(&self) -> Result<(), String> {
        let mut attempt = self.begin_attempt().await;
        let result = attempt.run(self.connect_in_attempt(true)).await;
        if result.is_err() {
            self.rollback_connection().await;
        }
        attempt.finish();
        if result.is_ok() {
            info!(event = "webrtc_reconnect_success", "WebRTC reconnected");
        }
        result
    }

    pub async fn auto_reconnect(&self, max_retries: u32) -> bool {
        if self.intentional_disconnect.load(Ordering::Relaxed) {
            return false;
        }

        for attempt in 0..max_retries {
            if self.intentional_disconnect.load(Ordering::Relaxed) {
                return false;
            }

            let mut attempt_guard = self.begin_attempt().await;
            if self.intentional_disconnect.load(Ordering::Relaxed) {
                attempt_guard.finish();
                return false;
            }
            let result = attempt_guard.run(self.connect_in_attempt(true)).await;
            if result.is_err() {
                self.rollback_connection().await;
            }
            attempt_guard.finish();
            if result.is_ok() {
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
    use crate::protocol::ports::PortResult;
    use std::future::Future;
    use std::pin::Pin;

    struct MockSignalingPort {
        called: Arc<AtomicBool>,
    }

    impl SignalingPort for MockSignalingPort {
        fn exchange_sdp<'a>(
            &'a self,
            _ip: &'a str,
            _offer: &'a str,
        ) -> Pin<Box<dyn Future<Output = PortResult<String>> + Send + 'a>> {
            self.called.store(true, Ordering::Relaxed);
            Box::pin(async { Ok(r#"{"type":"answer","sdp":"mock_sdp"}"#.to_string()) })
        }
    }

    struct MockRtcEnginePort {
        prepared: Arc<AtomicBool>,
        applied: Arc<AtomicBool>,
        closed: Arc<AtomicBool>,
    }

    impl RtcEnginePort for MockRtcEnginePort {
        fn prepare_offer<'a>(
            &'a self,
        ) -> Pin<Box<dyn Future<Output = PortResult<String>> + Send + 'a>> {
            self.prepared.store(true, Ordering::Relaxed);
            Box::pin(async { Ok("mock_offer_sdp".to_string()) })
        }

        fn apply_answer<'a>(
            &'a self,
            _answer_sdp: &'a str,
        ) -> Pin<Box<dyn Future<Output = PortResult<()>> + Send + 'a>> {
            self.applied.store(true, Ordering::Relaxed);
            Box::pin(async { Ok(()) })
        }

        fn close<'a>(&'a self) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>> {
            self.closed.store(true, Ordering::Relaxed);
            Box::pin(async {})
        }
    }

    #[tokio::test]
    async fn connect_logs_omit_address_and_userinfo() {
        if crate::test_support::run_isolated(
            "connection::service::tests::connect_logs_omit_address_and_userinfo",
        ) {
            return;
        }
        use tracing::instrument::WithSubscriber;
        #[derive(Clone, Default)]
        struct Capture(Arc<std::sync::Mutex<Vec<u8>>>);
        impl std::io::Write for Capture {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                self.0.lock().unwrap().extend_from_slice(bytes);
                Ok(bytes.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let output = Capture::default();
        let writer = output.clone();
        let subscriber = tracing_subscriber::fmt()
            .without_time()
            .with_ansi(false)
            .with_writer(move || writer.clone())
            .finish();
        let signaling = Arc::new(MockSignalingPort {
            called: Arc::new(AtomicBool::new(false)),
        });
        let engine = Arc::new(MockRtcEnginePort {
            prepared: Arc::new(AtomicBool::new(false)),
            applied: Arc::new(AtomicBool::new(false)),
            closed: Arc::new(AtomicBool::new(false)),
        });
        let service = ConnectionService::new(
            signaling,
            engine,
            WebRTCConnectionMethod::LocalSTA,
            Some(format!(
                "sentinel-user:sentinel-secret@{}.invalid",
                "private-address".repeat(1024)
            )),
        );
        service.connect().with_subscriber(subscriber).await.unwrap();
        let log = String::from_utf8(output.0.lock().unwrap().clone()).unwrap();
        assert!(log.contains("webrtc_connect_success"));
        for private in ["sentinel-user", "sentinel-secret", "private-address"] {
            assert!(
                !log.contains(private),
                "connection log exposed address/userinfo"
            );
        }
        assert!(log.len() < 1024);
    }

    #[tokio::test]
    async fn test_connect_success_local_sta() {
        let sig = Arc::new(MockSignalingPort {
            called: Arc::new(AtomicBool::new(false)),
        });
        let engine = Arc::new(MockRtcEnginePort {
            prepared: Arc::new(AtomicBool::new(false)),
            applied: Arc::new(AtomicBool::new(false)),
            closed: Arc::new(AtomicBool::new(false)),
        });

        let service = ConnectionService::new(
            sig.clone(),
            engine.clone(),
            WebRTCConnectionMethod::LocalSTA,
            Some("192.168.1.1".to_string()),
        );

        let res = service.connect().await;
        assert!(res.is_ok());
        assert!(sig.called.load(Ordering::Relaxed));
        assert!(engine.prepared.load(Ordering::Relaxed));
        assert!(engine.applied.load(Ordering::Relaxed));
        assert!(service.is_connected());
    }

    #[tokio::test]
    async fn test_disconnect() {
        let sig = Arc::new(MockSignalingPort {
            called: Arc::new(AtomicBool::new(false)),
        });
        let engine = Arc::new(MockRtcEnginePort {
            prepared: Arc::new(AtomicBool::new(false)),
            applied: Arc::new(AtomicBool::new(false)),
            closed: Arc::new(AtomicBool::new(false)),
        });

        let service = ConnectionService::new(
            sig,
            engine.clone(),
            WebRTCConnectionMethod::LocalSTA,
            Some("192.168.1.1".to_string()),
        );

        service.disconnect().await;
        assert!(engine.closed.load(Ordering::Relaxed));
        assert!(!service.is_connected());
        assert!(service.intentional_disconnect());
    }

    #[tokio::test]
    async fn test_reconnect() {
        let sig = Arc::new(MockSignalingPort {
            called: Arc::new(AtomicBool::new(false)),
        });
        let engine = Arc::new(MockRtcEnginePort {
            prepared: Arc::new(AtomicBool::new(false)),
            applied: Arc::new(AtomicBool::new(false)),
            closed: Arc::new(AtomicBool::new(false)),
        });

        let service = ConnectionService::new(
            sig,
            engine.clone(),
            WebRTCConnectionMethod::LocalSTA,
            Some("192.168.1.1".to_string()),
        );

        // initial state
        assert!(!service.is_connected());

        // simulate reconnect calling close and then connect
        let res = service.reconnect().await;
        assert!(res.is_ok());
        assert!(engine.closed.load(Ordering::Relaxed));
        assert!(engine.prepared.load(Ordering::Relaxed));
        assert!(engine.applied.load(Ordering::Relaxed));
        assert!(service.is_connected());
    }
}

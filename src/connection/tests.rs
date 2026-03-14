use super::*;
use crate::protocol::constants::WebRTCConnectionMethod;
use crate::protocol::ports::{PortResult, RtcEnginePort, SignalingPort};
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

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

// ── Remote connection ──────────────────────────────────────────────────

/// Remote connection method returns "not implemented" error.
#[tokio::test]
async fn remote_connection_returns_not_implemented_error() {
    let signaling = Arc::new(MockSignaling::default());
    let engine = Arc::new(MockEngine::default());
    let service = ConnectionService::new(signaling, engine, WebRTCConnectionMethod::Remote, None);

    let result = service.connect().await;

    assert!(result.is_err());
    assert!(result.unwrap_err().contains("not implemented"));
}

// ── reconnect ──────────────────────────────────────────────────────────

/// reconnect closes engine then reconnects, answer_history has 2 entries.
#[tokio::test]
async fn reconnect_closes_then_reconnects() {
    let signaling = Arc::new(MockSignaling::default());
    let engine = Arc::new(MockEngine::default());
    let service = ConnectionService::new(
        signaling.clone(),
        engine.clone(),
        WebRTCConnectionMethod::LocalSTA,
        Some("10.0.0.1".to_string()),
    );

    service.connect().await.unwrap();
    assert_eq!(engine.answer_history.lock().unwrap().len(), 1);

    service.reconnect().await.unwrap();

    assert_eq!(*engine.closed_count.lock().unwrap(), 1, "should close once");
    assert_eq!(
        engine.answer_history.lock().unwrap().len(),
        2,
        "should have 2 answers"
    );
    assert!(service.is_connected());
}

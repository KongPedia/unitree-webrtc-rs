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
    fail_apply: bool,
    stall_apply: bool,
    close_barrier: Option<(Arc<tokio::sync::Notify>, Arc<tokio::sync::Notify>)>,
}

impl MockEngine {
    fn with_prepare_fail_count(fail_count: u32) -> Self {
        Self {
            closed_count: Arc::new(Mutex::new(0)),
            answer_history: Arc::new(Mutex::new(Vec::new())),
            fail_prepare_count: Arc::new(Mutex::new(fail_count)),
            fail_apply: false,
            stall_apply: false,
            close_barrier: None,
        }
    }
}

impl RtcEnginePort for MockEngine {
    fn timeout_diagnostic(&self) -> Option<String> {
        assert_eq!(
            *self.closed_count.lock().unwrap(),
            0,
            "snapshot must precede rollback"
        );
        self.stall_apply.then(|| {
            "peer=connecting, ice=checking, dtls=new, sctp=connecting, channel=connecting".into()
        })
    }

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
            if self.stall_apply {
                std::future::pending::<()>().await;
            }
            if self.fail_apply {
                return Err("apply failed".to_string());
            }
            self.answer_history
                .lock()
                .unwrap()
                .push(answer_sdp.to_string());
            Ok(())
        })
    }

    fn close<'a>(&'a self) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>> {
        Box::pin(async move {
            *self.closed_count.lock().unwrap() += 1;
            if let Some((started, release)) = &self.close_barrier {
                started.notify_one();
                release.notified().await;
            }
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

#[tokio::test]
async fn failed_signaling_rolls_back_engine_and_connected_state() {
    let signaling = Arc::new(MockSignaling::with_fail_count(1));
    let engine = Arc::new(MockEngine::default());
    let service = ConnectionService::new(
        signaling,
        engine.clone(),
        WebRTCConnectionMethod::LocalSTA,
        Some("10.0.0.1".into()),
    );
    assert!(service.connect().await.is_err());
    assert!(!service.is_connected());
    assert_eq!(*engine.closed_count.lock().unwrap(), 1);
}

#[tokio::test]
async fn failed_prepare_rolls_back_engine() {
    let engine = Arc::new(MockEngine::with_prepare_fail_count(1));
    let service = ConnectionService::new(
        Arc::new(MockSignaling::default()),
        engine.clone(),
        WebRTCConnectionMethod::LocalSTA,
        Some("10.0.0.1".into()),
    );
    assert!(service.connect().await.is_err());
    assert_eq!(*engine.closed_count.lock().unwrap(), 1);
}

#[tokio::test]
async fn failed_answer_rolls_back_engine() {
    let engine = Arc::new(MockEngine {
        fail_apply: true,
        ..Default::default()
    });
    let service = ConnectionService::new(
        Arc::new(MockSignaling::default()),
        engine.clone(),
        WebRTCConnectionMethod::LocalSTA,
        Some("10.0.0.1".into()),
    );
    assert!(service.connect().await.is_err());
    assert!(!service.is_connected());
    assert_eq!(*engine.closed_count.lock().unwrap(), 1);
}

struct BlockingSignaling(tokio::sync::Notify);
impl SignalingPort for BlockingSignaling {
    fn exchange_sdp<'a>(
        &'a self,
        _: &'a str,
        _: &'a str,
    ) -> Pin<Box<dyn Future<Output = PortResult<String>> + Send + 'a>> {
        Box::pin(async move {
            self.0.notify_one();
            std::future::pending().await
        })
    }
}

#[tokio::test]
async fn cancelling_connect_closes_partial_session() {
    let signaling = Arc::new(BlockingSignaling(tokio::sync::Notify::new()));
    let engine = Arc::new(MockEngine::default());
    let service = Arc::new(ConnectionService::new(
        signaling.clone(),
        engine.clone(),
        WebRTCConnectionMethod::LocalSTA,
        Some("10.0.0.1".into()),
    ));
    let task = tokio::spawn(async move { service.connect().await });
    signaling.0.notified().await;
    task.abort();
    task.await.unwrap_err();
    tokio::time::timeout(std::time::Duration::from_secs(1), async {
        while *engine.closed_count.lock().unwrap() == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(*engine.closed_count.lock().unwrap(), 1);
}

struct FirstBlockingSignaling {
    started: tokio::sync::Notify,
    calls: std::sync::atomic::AtomicUsize,
}
impl SignalingPort for FirstBlockingSignaling {
    fn exchange_sdp<'a>(
        &'a self,
        _: &'a str,
        _: &'a str,
    ) -> Pin<Box<dyn Future<Output = PortResult<String>> + Send + 'a>> {
        Box::pin(async move {
            if self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0 {
                self.started.notify_one();
                std::future::pending::<()>().await;
            }
            Ok("answer".into())
        })
    }
}

#[tokio::test]
async fn cancelled_cleanup_holds_gate_until_close_before_next_attempt() {
    let close_started = Arc::new(tokio::sync::Notify::new());
    let close_release = Arc::new(tokio::sync::Notify::new());
    let engine = Arc::new(MockEngine {
        close_barrier: Some((close_started.clone(), close_release.clone())),
        ..Default::default()
    });
    let signaling = Arc::new(FirstBlockingSignaling {
        started: tokio::sync::Notify::new(),
        calls: std::sync::atomic::AtomicUsize::new(0),
    });
    let service = Arc::new(ConnectionService::new(
        signaling.clone(),
        engine.clone(),
        WebRTCConnectionMethod::LocalSTA,
        Some("10.0.0.1".into()),
    ));
    let first_service = Arc::clone(&service);
    let first = tokio::spawn(async move { first_service.connect().await });
    signaling.started.notified().await;
    first.abort();
    first.await.unwrap_err();
    tokio::time::timeout(std::time::Duration::from_secs(1), close_started.notified())
        .await
        .unwrap();
    let next_service = Arc::clone(&service);
    let next = tokio::spawn(async move { next_service.connect().await });
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    assert_eq!(
        signaling.calls.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "new session cannot start while cancelled cleanup is pending"
    );
    close_release.notify_one();
    tokio::time::timeout(std::time::Duration::from_secs(1), next)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(*engine.closed_count.lock().unwrap(), 1);
    assert!(
        service.is_connected(),
        "late cleanup cannot close new session"
    );
}

#[tokio::test]
async fn explicit_disconnect_interrupts_blocked_signaling() {
    let signaling = Arc::new(BlockingSignaling(tokio::sync::Notify::new()));
    let service = Arc::new(ConnectionService::new(
        Arc::clone(&signaling),
        Arc::new(MockEngine::default()),
        WebRTCConnectionMethod::LocalSTA,
        Some("10.0.0.1".into()),
    ));
    let connecting = {
        let service = Arc::clone(&service);
        tokio::spawn(async move { service.connect().await })
    };
    signaling.0.notified().await;
    tokio::time::timeout(std::time::Duration::from_millis(200), service.disconnect())
        .await
        .expect("disconnect must cancel signaling before waiting for the gate");
    assert!(connecting.await.unwrap().unwrap_err().contains("cancel"));
    assert!(!service.is_connected());
    assert!(service.intentional_disconnect());
}

#[tokio::test]
async fn disconnect_invalidates_connect_queued_before_the_session_gate() {
    let signaling = Arc::new(MockSignaling::default());
    let service = ConnectionService::new(
        Arc::clone(&signaling),
        Arc::new(MockEngine::default()),
        WebRTCConnectionMethod::LocalSTA,
        Some("10.0.0.1".into()),
    );
    let gate = service.lock_session().await;
    let (result, ()) = tokio::join!(
        biased;
        service.connect(),
        async {
            service.request_disconnect();
            drop(gate);
            service.disconnect().await;
        }
    );
    assert!(result.unwrap_err().contains("cancel"));
    assert!(signaling.calls.lock().unwrap().is_empty());
    assert!(service.intentional_disconnect());
    service.connect().await.unwrap();
    assert!(
        service.is_connected(),
        "a later explicit connect may start a fresh attempt"
    );
}

#[tokio::test]
async fn connect_deadline_interrupts_blocked_signaling() {
    let signaling = Arc::new(BlockingSignaling(tokio::sync::Notify::new()));
    let service = ConnectionService::new(
        signaling,
        Arc::new(MockEngine::default()),
        WebRTCConnectionMethod::LocalSTA,
        Some("10.0.0.1".into()),
    );
    let result = tokio::time::timeout(std::time::Duration::from_secs(17), service.connect())
        .await
        .unwrap();
    assert_eq!(result.unwrap_err(), "Connection attempt timed out");
    assert!(!service.is_connected());
}

#[tokio::test]
async fn connect_deadline_reports_transport_snapshot_before_rollback() {
    let engine = Arc::new(MockEngine {
        stall_apply: true,
        ..Default::default()
    });
    let service = ConnectionService::new(
        Arc::new(MockSignaling::default()),
        engine.clone(),
        WebRTCConnectionMethod::LocalSTA,
        Some("secret-address.invalid".into()),
    );
    let error = tokio::time::timeout(std::time::Duration::from_secs(17), service.connect())
        .await
        .unwrap()
        .unwrap_err();
    assert!(error.contains("Connection attempt timed out"));
    assert!(
        error.contains(
            "peer=connecting, ice=checking, dtls=new, sctp=connecting, channel=connecting"
        ),
        "{error}"
    );
    assert!(!error.contains("secret-address"));
    assert_eq!(*engine.closed_count.lock().unwrap(), 1);
    assert!(!service.is_connected());
}

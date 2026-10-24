//! An external consumer: no crate-private helpers or Python runtime initialization.
use crossbeam_channel::Sender;
use serde_json::{json, Value};
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use unitree_webrtc_rs::connection::{SessionOptions, UnitreeSession};
use unitree_webrtc_rs::protocol::constants::WebRTCConnectionMethod;
use unitree_webrtc_rs::protocol::events::EventReceiver;
use unitree_webrtc_rs::protocol::models::{CallbackEvent, DcMessage, IngressMessage};
use unitree_webrtc_rs::protocol::ports::{
    DataChannelPort, PortResult, RtcEnginePort, SignalingPort,
};

type PortFuture<'a, T> = Pin<Box<dyn Future<Output = PortResult<T>> + Send + 'a>>;
#[derive(Default)]
struct FakeRobot {
    incoming: Mutex<Option<Sender<IngressMessage>>>,
    generation: AtomicU64,
    ready: AtomicBool,
    validate: AtomicBool,
    fail_signaling: AtomicUsize,
    closes: AtomicUsize,
    block_signaling: AtomicBool,
    reject_generation: AtomicBool,
    prepared: AtomicBool,
    signaling_started: tokio::sync::Notify,
    requests_sent: AtomicUsize,
}
impl FakeRobot {
    fn inject(&self, generation: u64, message: Value) {
        self.incoming
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .try_send(DcMessage::Text(message.to_string()).for_generation(generation))
            .expect("bounded offline fixture ingress must have room");
    }

    async fn wait_for_requests(&self, expected: usize) {
        tokio::time::timeout(Duration::from_secs(2), async {
            while self.requests_sent.load(Ordering::SeqCst) < expected {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("requests must reach the adapter before the next test action");
    }
}
impl SignalingPort for FakeRobot {
    fn exchange_sdp<'a>(&'a self, _: &'a str, _: &'a str) -> PortFuture<'a, String> {
        Box::pin(async move {
            if self.block_signaling.load(Ordering::SeqCst) {
                self.signaling_started.notify_one();
                std::future::pending::<()>().await;
            }
            if self
                .fail_signaling
                .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1))
                .is_ok()
            {
                return Err("fixture signaling failure".into());
            }
            Ok("answer".into())
        })
    }
}
impl RtcEnginePort for FakeRobot {
    fn prepare_offer<'a>(&'a self) -> PortFuture<'a, String> {
        self.prepared.store(true, Ordering::SeqCst);
        Box::pin(async { Ok("offer".into()) })
    }
    fn apply_answer<'a>(&'a self, _: &'a str) -> PortFuture<'a, ()> {
        Box::pin(async move {
            self.ready.store(true, Ordering::SeqCst);
            if self.validate.load(Ordering::SeqCst) {
                self.incoming
                    .lock()
                    .unwrap()
                    .as_ref()
                    .unwrap()
                    .send(
                        DcMessage::Text(r#"{"type":"validation","data":"Validation Ok."}"#.into())
                            .for_generation(self.generation.load(Ordering::SeqCst)),
                    )
                    .unwrap();
            }
            Ok(())
        })
    }
    fn close<'a>(&'a self) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>> {
        Box::pin(async move {
            self.ready.store(false, Ordering::SeqCst);
            self.closes.fetch_add(1, Ordering::SeqCst);
        })
    }
}
impl DataChannelPort for FakeRobot {
    fn send_text(&self, text: &str) -> PortResult<()> {
        let message: Value = serde_json::from_str(text).unwrap();
        if message["type"] == "req" {
            self.requests_sent.fetch_add(1, Ordering::SeqCst);
        }
        Ok(())
    }
    fn send_binary(&self, _: &[u8]) -> PortResult<()> {
        Ok(())
    }
    fn set_message_sender(&self, sender: Sender<IngressMessage>) -> PortResult<()> {
        *self.incoming.lock().unwrap() = Some(sender);
        Ok(())
    }
    fn set_ingress_generation(&self, generation: u64) -> PortResult<()> {
        if self.reject_generation.load(Ordering::SeqCst) {
            return Err("fixture cannot isolate ingress generations".into());
        }
        self.generation.store(generation, Ordering::SeqCst);
        Ok(())
    }
    fn ready_state(&self) -> &'static str {
        if self.ready.load(Ordering::SeqCst) {
            "open"
        } else {
            "closed"
        }
    }
}
fn session() -> (
    Arc<UnitreeSession<FakeRobot, FakeRobot>>,
    Arc<FakeRobot>,
    EventReceiver,
) {
    session_with_timeout(Duration::from_millis(10))
}

fn session_with_timeout(
    validation_timeout: Duration,
) -> (
    Arc<UnitreeSession<FakeRobot, FakeRobot>>,
    Arc<FakeRobot>,
    EventReceiver,
) {
    let robot = Arc::new(FakeRobot::default());
    robot.validate.store(true, Ordering::SeqCst);
    let mut options =
        SessionOptions::new(WebRTCConnectionMethod::LocalSTA, Some("10.0.0.1".into()));
    options.validation_timeout = validation_timeout;
    options.lidar_workers = 1;
    let engine = Arc::clone(&robot);
    let (session, events) =
        UnitreeSession::with_adapters(options, robot.clone(), move |incoming, _events| {
            engine.set_message_sender(incoming).unwrap();
            engine
        })
        .unwrap();
    (Arc::new(session), robot, events)
}
#[tokio::test]
async fn public_session_validates_reconnects_and_respects_intentional_disconnect() {
    let (session, robot, _events) = session();
    session.connect().await.unwrap();
    assert!(session.is_connected());
    robot.validate.store(false, Ordering::SeqCst);
    assert!(session.reconnect().await.is_err());
    assert!(!session.is_connected());
    robot.validate.store(true, Ordering::SeqCst);
    robot.fail_signaling.store(1, Ordering::SeqCst);
    assert!(session.auto_reconnect(2).await);
    assert!(session.is_connected());
    session.disconnect().await;
    assert!(!session.is_connected());
    assert!(!session.auto_reconnect(2).await);
}
#[tokio::test]
async fn cancelling_external_session_connect_cleans_up_without_python() {
    let (session, robot, _events) = session();
    robot.block_signaling.store(true, Ordering::SeqCst);
    let owned = Arc::clone(&session);
    let task = tokio::spawn(async move { owned.connect().await });
    robot.signaling_started.notified().await;
    task.abort();
    task.await.unwrap_err();
    tokio::time::timeout(Duration::from_secs(1), async {
        while robot.closes.load(Ordering::SeqCst) == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert!(!session.is_connected());
}
#[test]
fn impossible_resource_settings_fail_before_allocation() {
    let mut options =
        SessionOptions::new(WebRTCConnectionMethod::LocalSTA, Some("10.0.0.1".into()));
    options.event_capacity = usize::MAX;
    let robot = Arc::new(FakeRobot::default());
    let result = UnitreeSession::<FakeRobot, FakeRobot>::with_adapters(options, robot, |_, _| {
        panic!("validation must precede adapter creation")
    });
    assert!(result.is_err());
}

#[test]
fn final_owner_drop_releases_event_producers_without_python() {
    let robot = Arc::new(FakeRobot::default());
    let mut options =
        SessionOptions::new(WebRTCConnectionMethod::LocalSTA, Some("10.0.0.1".into()));
    options.lidar_workers = 1;
    let engine = Arc::clone(&robot);
    let (session, events) = UnitreeSession::with_adapters(options, robot, move |incoming, _| {
        engine.set_message_sender(incoming).unwrap();
        engine
    })
    .unwrap();
    drop(session);
    assert!(matches!(
        events.recv_timeout(Duration::from_secs(2)),
        Err(crossbeam_channel::RecvTimeoutError::Disconnected)
    ));
}

#[tokio::test]
async fn external_consumer_receives_owned_topic_event() {
    let (session, robot, events) = session();
    session.connect().await.unwrap();
    session.datachannel().subscribe("fixture/topic").unwrap();
    robot
        .incoming
        .lock()
        .unwrap()
        .as_ref()
        .unwrap()
        .send(
            DcMessage::Text(r#"{"type":"msg","topic":"fixture/topic","data":{"value":42}}"#.into())
                .for_generation(robot.generation.load(Ordering::SeqCst)),
        )
        .unwrap();
    let event =
        tokio::task::spawn_blocking(move || events.recv_timeout(Duration::from_secs(1)).unwrap())
            .await
            .unwrap();
    assert!(
        matches!(event, CallbackEvent::TopicCallback {topic, payload} if topic == "fixture/topic" && payload["data"]["value"] == 42)
    );
    session.disconnect().await;
}

#[tokio::test]
async fn unsupported_ingress_generation_fails_before_offer_creation() {
    let (session, robot, _events) = session();
    robot.reject_generation.store(true, Ordering::SeqCst);
    let error = session.connect().await.unwrap_err();
    assert!(error.contains("cannot isolate ingress generations"));
    assert!(!robot.prepared.load(Ordering::SeqCst));
    assert!(!session.is_connected());
    assert_eq!(
        robot.closes.load(Ordering::SeqCst),
        1,
        "failed attempt must roll back"
    );
}

// Characterization: dropping epoch checks, cross-wiring session resources, or
// retaining background owners would break consumer-visible delivery/closure.
#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
async fn repeated_sessions_isolate_epoch_delivery_and_release_final_owners() {
    tokio::time::timeout(Duration::from_secs(15), async {
        let ready = Arc::new(tokio::sync::Barrier::new(3));
        let mut sessions = tokio::task::JoinSet::new();
        for session_id in 0..3 {
            let ready = Arc::clone(&ready);
            sessions.spawn(async move {
                let (session, robot, events) = session_with_timeout(Duration::from_secs(2));
                let weak_robot = Arc::downgrade(&robot);
                session.connect().await.unwrap();
                ready.wait().await;
                let mut stale_generation = None;
                for round in 0..8 {
                    assert!(session.is_connected());
                    session
                        .datachannel()
                        .subscribe("fixture/shared-topic")
                        .unwrap();
                    let current_generation = robot.generation.load(Ordering::SeqCst);
                    if let Some(stale_generation) = stale_generation {
                        robot.inject(
                            stale_generation,
                            json!({
                                "type": "msg", "topic": "fixture/shared-topic",
                                "data": {"session": session_id, "round": "stale"}
                            }),
                        );
                    }
                    robot.inject(
                        current_generation,
                        json!({
                            "type": "msg", "topic": "fixture/shared-topic",
                            "data": {"session": session_id, "round": round}
                        }),
                    );
                    let event = tokio::time::timeout(Duration::from_secs(2), async {
                        loop {
                            match events.try_recv() {
                                Ok(event) => break event,
                                Err(crossbeam_channel::TryRecvError::Empty) => {
                                    tokio::task::yield_now().await;
                                }
                                Err(error) => panic!("live session lost event producers: {error}"),
                            }
                        }
                    })
                    .await
                    .unwrap();
                    assert!(
                        matches!(event, CallbackEvent::TopicCallback {topic, payload}
                        if topic == "fixture/shared-topic"
                            && payload["data"] == json!({"session": session_id, "round": round}))
                    );
                    stale_generation = Some(current_generation);
                    if round % 2 == 0 {
                        session.disconnect().await;
                        assert!(!session.is_connected());
                        session.connect().await.unwrap();
                    } else {
                        session.reconnect().await.unwrap();
                    }
                }
                session.disconnect().await;
                drop(session);
                drop(robot);
                tokio::time::timeout(Duration::from_secs(2), async {
                    loop {
                        match events.try_recv() {
                            Err(crossbeam_channel::TryRecvError::Disconnected) => break,
                            Err(crossbeam_channel::TryRecvError::Empty) => {
                                tokio::task::yield_now().await;
                            }
                            Ok(event) => {
                                panic!("stale delivery survived final owner release: {event:?}")
                            }
                        }
                    }
                    while weak_robot.upgrade().is_some() {
                        tokio::task::yield_now().await;
                    }
                })
                .await
                .expect("final owner release must close producers and release adapters");
            });
        }
        while let Some(result) = sessions.join_next().await {
            result.unwrap();
        }
    })
    .await
    .expect("bounded three-session/eight-round characterization must finish");
}

fn spawn_request(
    session: &UnitreeSession<FakeRobot, FakeRobot>,
    id: i64,
) -> tokio::task::JoinHandle<PortResult<Value>> {
    let channel = session.datachannel();
    tokio::spawn(async move {
        channel
            .publish_request_new(
                "rt/api/sport/request",
                json!({"api_id": 1001, "id": id}),
                Some(10.0),
            )
            .await
    })
}

fn rpc_response(id: i64, data: &str) -> Value {
    json!({
        "type": "res", "topic": "rt/api/sport/response",
        "data": {
            "header": {"identity": {"id": id, "api_id": 1001}, "status": {"code": 0}},
            "data": data
        }
    })
}

// Characterization: missing admission, cancellation cleanup, or reset completion
// would cause an extra send, failed replacement, timeout, or stale successful RPC.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn repeated_rpc_saturation_cancellation_and_reset_recover_independent_sessions() {
    tokio::time::timeout(Duration::from_secs(15), async {
        let ready = Arc::new(tokio::sync::Barrier::new(2));
        let mut sessions = tokio::task::JoinSet::new();
        for _ in 0..2 {
            let ready = Arc::clone(&ready);
            sessions.spawn(async move {
                let (session, robot, _events) = session_with_timeout(Duration::from_secs(2));
                session.connect().await.unwrap();
                ready.wait().await;
                for _ in 0..3 {
                    let baseline = robot.requests_sent.load(Ordering::SeqCst);
                    let mut pending = (1000..1128)
                        .map(|id| spawn_request(&session, id))
                        .collect::<Vec<_>>();
                    robot.wait_for_requests(baseline + 128).await;
                    let error = session
                        .datachannel()
                        .publish_request_new(
                            "rt/api/sport/request",
                            json!({"api_id": 1001, "id": 9999}),
                            Some(1.0),
                        )
                        .await
                        .unwrap_err();
                    assert!(error.contains("in-flight request limit"));
                    assert_eq!(
                        robot.requests_sent.load(Ordering::SeqCst),
                        baseline + 128,
                        "overflow must fail before transport enqueue"
                    );
                    for cancelled in pending.drain(..64) {
                        cancelled.abort();
                        assert!(cancelled.await.unwrap_err().is_cancelled());
                    }
                    let mut replacements = (1000..1064)
                        .map(|id| spawn_request(&session, id))
                        .collect::<Vec<_>>();
                    robot.wait_for_requests(baseline + 192).await;
                    let generation = robot.generation.load(Ordering::SeqCst);
                    robot.inject(generation, rpc_response(1000, "recovered"));
                    let recovered = replacements.remove(0).await.unwrap().unwrap();
                    assert_eq!(recovered["data"]["data"], "recovered");
                    pending.extend(replacements);
                    session.reconnect().await.unwrap();
                    for reset in pending {
                        assert!(reset.await.unwrap().unwrap_err().contains("session reset"));
                    }
                    // Reuse an old identity only after reset. A late old response
                    // must not resolve the current request, even with matching wire fields.
                    let recovered = spawn_request(&session, 1000);
                    robot.wait_for_requests(baseline + 193).await;
                    robot.inject(generation, rpc_response(1000, "stale"));
                    robot.inject(
                        robot.generation.load(Ordering::SeqCst),
                        rpc_response(1000, "fresh"),
                    );
                    assert_eq!(recovered.await.unwrap().unwrap()["data"]["data"], "fresh");
                }
                session.disconnect().await;
            });
        }
        while let Some(result) = sessions.join_next().await {
            result.unwrap();
        }
    })
    .await
    .expect("bounded two-session/three-round RPC characterization must finish");
}

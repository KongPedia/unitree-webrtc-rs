//! An external consumer: no crate-private helpers or Python runtime initialization.
use crossbeam_channel::Sender;
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
    fn send_text(&self, _: &str) -> PortResult<()> {
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
    let robot = Arc::new(FakeRobot::default());
    robot.validate.store(true, Ordering::SeqCst);
    let mut options =
        SessionOptions::new(WebRTCConnectionMethod::LocalSTA, Some("10.0.0.1".into()));
    options.validation_timeout = Duration::from_millis(10);
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

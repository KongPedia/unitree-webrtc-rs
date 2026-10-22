use crossbeam_channel::bounded;
use serde_json::{json, Value};
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use unitree_webrtc_rs::connection::ConnectionService;
use unitree_webrtc_rs::datachannel::core::DataChannelService;
use unitree_webrtc_rs::datachannel::lidar;
use unitree_webrtc_rs::protocol::constants::{rtc_topic, sport_cmd, WebRTCConnectionMethod};
use unitree_webrtc_rs::protocol::events::{event_channel, EventReceiver};
use unitree_webrtc_rs::protocol::models::{CallbackEvent, DcMessage, IngressMessage};
use unitree_webrtc_rs::protocol::ports::{
    DataChannelPort, PortResult, RtcEnginePort, SignalingPort,
};

/// A consolidated mock system that satisfies both Signaling, RTC, and DataChannel ports.
/// This allows us to simulate the entire environment that the ConnectionService and DataChannelService need.
struct MockEnvironment {
    signaling_called: Arc<AtomicBool>,
    engine_prepared: Arc<AtomicBool>,
    engine_applied: Arc<AtomicBool>,
    engine_closed: Arc<AtomicBool>,
    dc_ready_state: &'static str,
    generation: AtomicU64,
    respond_to_requests: AtomicBool,
    dc_sent_messages: Arc<std::sync::Mutex<Vec<String>>>,
    dc_sender: std::sync::Mutex<Option<crossbeam_channel::Sender<IngressMessage>>>,
}

impl MockEnvironment {
    fn new(ready_state: &'static str) -> Self {
        Self {
            signaling_called: Arc::new(AtomicBool::new(false)),
            engine_prepared: Arc::new(AtomicBool::new(false)),
            engine_applied: Arc::new(AtomicBool::new(false)),
            engine_closed: Arc::new(AtomicBool::new(false)),
            dc_ready_state: ready_state,
            generation: AtomicU64::new(0),
            respond_to_requests: AtomicBool::new(false),
            dc_sent_messages: Arc::new(std::sync::Mutex::new(Vec::new())),
            dc_sender: std::sync::Mutex::new(None),
        }
    }

    fn sent_message(&self, message_type: &str, topic: &str) -> Value {
        self.dc_sent_messages
            .lock()
            .unwrap()
            .iter()
            .rev()
            .map(|message| serde_json::from_str::<Value>(message).unwrap())
            .find(|message| message["type"] == message_type && message["topic"] == topic)
            .expect("Expected matching outbound envelope")
    }

    fn interleave_heartbeat(&self) {
        // A background frame must not change which application envelope is asserted.
        self.send_text(r#"{"type":"heartbeat","topic":"","data":{"time":1}}"#)
            .unwrap();
    }

    fn push_incoming_message(&self, message: &str) {
        if let Some(sender) = self.dc_sender.lock().unwrap().as_ref() {
            let _ = sender.send(
                DcMessage::Text(message.to_string())
                    .for_generation(self.generation.load(Ordering::SeqCst)),
            );
        }
    }
}

impl SignalingPort for MockEnvironment {
    fn exchange_sdp<'a>(
        &'a self,
        _ip: &'a str,
        _offer: &'a str,
    ) -> Pin<Box<dyn Future<Output = PortResult<String>> + Send + 'a>> {
        self.signaling_called.store(true, Ordering::Relaxed);
        Box::pin(async { Ok(r#"{"type":"answer","sdp":"mock_sdp"}"#.to_string()) })
    }
}

impl RtcEnginePort for MockEnvironment {
    fn prepare_offer<'a>(
        &'a self,
    ) -> Pin<Box<dyn Future<Output = PortResult<String>> + Send + 'a>> {
        self.engine_prepared.store(true, Ordering::Relaxed);
        Box::pin(async { Ok("mock_offer_sdp".to_string()) })
    }

    fn apply_answer<'a>(
        &'a self,
        _answer_sdp: &'a str,
    ) -> Pin<Box<dyn Future<Output = PortResult<()>> + Send + 'a>> {
        self.engine_applied.store(true, Ordering::Relaxed);
        Box::pin(async { Ok(()) })
    }

    fn close<'a>(&'a self) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>> {
        self.engine_closed.store(true, Ordering::Relaxed);
        Box::pin(async {})
    }
}

impl DataChannelPort for MockEnvironment {
    fn send_text(&self, message: &str) -> PortResult<()> {
        self.dc_sent_messages
            .lock()
            .unwrap()
            .push(message.to_string());
        let envelope: Value = serde_json::from_str(message).unwrap();
        if envelope["type"] == "req" && self.respond_to_requests.load(Ordering::Relaxed) {
            self.interleave_heartbeat();
            self.push_incoming_message(
                &json!({
                    "type": "res",
                    "topic": "rt/api/sport/response",
                    "data": {
                        "header": {
                            "identity": envelope["data"]["header"]["identity"],
                            "status": {"code": 0}
                        },
                        "data": "mock_state"
                    }
                })
                .to_string(),
            );
        }
        Ok(())
    }

    fn send_binary(&self, _bytes: &[u8]) -> PortResult<()> {
        Ok(())
    }

    fn set_message_sender(
        &self,
        sender: crossbeam_channel::Sender<IngressMessage>,
    ) -> PortResult<()> {
        *self.dc_sender.lock().unwrap() = Some(sender);
        Ok(())
    }

    fn set_ingress_generation(&self, generation: u64) -> PortResult<()> {
        self.generation.store(generation, Ordering::SeqCst);
        Ok(())
    }

    fn ready_state(&self) -> &'static str {
        self.dc_ready_state
    }
}

/// Helper function to configure the services exactly like the Python bridge does.
#[allow(clippy::type_complexity)]
async fn setup_test_environment() -> (
    Arc<ConnectionService<MockEnvironment, MockEnvironment>>,
    Arc<DataChannelService<MockEnvironment>>,
    Arc<MockEnvironment>,
    EventReceiver,
) {
    let mock_env = Arc::new(MockEnvironment::new("open")); // Assuming "open" for tests
    let (incoming_tx, incoming_rx) = bounded::<IngressMessage>(100);
    let (callback_tx, callback_rx) = event_channel(100).unwrap();

    // MockEnvironment captures the incoming sender to simulate WebRTC returning data
    mock_env.set_message_sender(incoming_tx).unwrap();

    let conn_service = Arc::new(ConnectionService::new(
        mock_env.clone(),
        mock_env.clone(),
        WebRTCConnectionMethod::LocalSTA,
        Some("192.168.1.1".to_string()),
    ));

    let lidar_pool = lidar::create_worker_pool(callback_tx.clone());
    let dc_service = Arc::new(DataChannelService::new(
        mock_env.clone(),
        incoming_rx,
        callback_tx,
        lidar_pool,
        false, // not remote connection
    ));

    (conn_service, dc_service, mock_env, callback_rx)
}

#[tokio::test]
async fn test_sport_mode_example_flow() {
    let (conn, dc, env, _cb_rx) = setup_test_environment().await;

    // 1. Connect
    let connect_res = conn.connect().await;
    assert!(connect_res.is_ok(), "Connection should succeed");

    // 2. Simulate datachannel opening (validation message)
    env.push_incoming_message(r#"{"data": "Validation Ok.", "type": "validation"}"#);

    // Wait for validation to be processed
    let dc_wait: PortResult<()> = dc.wait_datachannel_open(1.0).await;
    assert!(dc_wait.is_ok(), "Datachannel should open successfully");

    // Reply only after observing the request, without relying on scheduler timing.
    env.respond_to_requests.store(true, Ordering::Relaxed);
    let topic = rtc_topic()["SPORT_MOD"];
    let api_id = sport_cmd()["GetState"];
    let response = dc
        .publish_request_new(topic, json!({"api_id": api_id, "id": 1}), Some(2.0))
        .await
        .expect("Matching sport response should complete the request");
    assert_eq!(response["data"]["header"]["status"]["code"], 0);
    assert_eq!(response["data"]["data"], "mock_state");

    env.interleave_heartbeat();
    let request = env.sent_message("req", topic);
    assert_eq!(request["data"]["header"]["identity"]["api_id"], api_id);
    assert_eq!(request["data"]["header"]["identity"]["id"], 1);
    assert_eq!(request["data"]["parameter"], "");
}

#[tokio::test]
async fn test_sport_mode_state_subscription_flow() {
    let (conn, dc, env, cb_rx) = setup_test_environment().await;

    // 1. Connect
    conn.connect().await.unwrap();

    // 2. Subscribe to LF_SPORT_MOD_STATE (Mimic `sport_mode_state.py`)
    let topic = rtc_topic()["LF_SPORT_MOD_STATE"];
    let sub_res = dc.subscribe(topic);
    assert!(sub_res.is_ok());

    env.interleave_heartbeat();
    let subscription = env.sent_message("subscribe", topic);
    assert_eq!(subscription["topic"], topic);

    env.push_incoming_message(
        &json!({"type": "msg", "topic": topic, "data": {"mode": "standing"}}).to_string(),
    );

    // 4. Verify message is routed to Python's callback channel
    let evt = cb_rx.recv_timeout(std::time::Duration::from_secs(1));
    assert!(evt.is_ok(), "Should receive callback event");

    match evt.unwrap() {
        CallbackEvent::TopicCallback { topic, payload } => {
            assert_eq!(topic, rtc_topic()["LF_SPORT_MOD_STATE"]);
            assert_eq!(payload["data"]["mode"], "standing");
        }
        _ => panic!("Expected TopicCallback"),
    }
}

#[tokio::test]
async fn test_video_stream_flow() {
    let (conn, dc, env, _cb_rx) = setup_test_environment().await;

    // 1. Connect
    conn.connect().await.unwrap();

    // 2. Enable Video Channel (Mimic `video_stream.py`)
    let switch_res = dc.switch_video_channel(true);
    assert!(switch_res.is_ok());

    env.interleave_heartbeat();
    let video_on = env.sent_message("vid", "");
    assert_eq!(video_on["data"], "on");

    // Disable Video Channel
    let switch_off_res = dc.switch_video_channel(false);
    assert!(switch_off_res.is_ok());

    env.interleave_heartbeat();
    let video_off = env.sent_message("vid", "");
    assert_eq!(video_off["data"], "off");

    // 3. Disconnect
    conn.disconnect().await;
    assert!(env.engine_closed.load(Ordering::Relaxed));
}

use crossbeam_channel::bounded;
use serde_json::json;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use unitree_webrtc_rs::connection::ConnectionService;
use unitree_webrtc_rs::datachannel::core::DataChannelService;
use unitree_webrtc_rs::datachannel::lidar;
use unitree_webrtc_rs::protocol::constants::WebRTCConnectionMethod;
use unitree_webrtc_rs::protocol::models::{CallbackEvent, DcMessage};
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
    dc_sent_messages: Arc<std::sync::Mutex<Vec<String>>>,
    dc_sender: std::sync::Mutex<Option<crossbeam_channel::Sender<DcMessage>>>,
}

impl MockEnvironment {
    fn new(ready_state: &'static str) -> Self {
        Self {
            signaling_called: Arc::new(AtomicBool::new(false)),
            engine_prepared: Arc::new(AtomicBool::new(false)),
            engine_applied: Arc::new(AtomicBool::new(false)),
            engine_closed: Arc::new(AtomicBool::new(false)),
            dc_ready_state: ready_state,
            dc_sent_messages: Arc::new(std::sync::Mutex::new(Vec::new())),
            dc_sender: std::sync::Mutex::new(None),
        }
    }

    fn push_incoming_message(&self, message: &str) {
        if let Some(sender) = self.dc_sender.lock().unwrap().as_ref() {
            let _ = sender.send(DcMessage::Text(message.to_string()));
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
        Ok(())
    }

    fn send_binary(&self, _bytes: &[u8]) -> PortResult<()> {
        Ok(())
    }

    fn set_message_sender(&self, sender: crossbeam_channel::Sender<DcMessage>) -> PortResult<()> {
        *self.dc_sender.lock().unwrap() = Some(sender);
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
    crossbeam_channel::Receiver<CallbackEvent>,
) {
    let mock_env = Arc::new(MockEnvironment::new("open")); // Assuming "open" for tests
    let (incoming_tx, incoming_rx) = bounded::<DcMessage>(100);
    let (callback_tx, callback_rx) = bounded::<CallbackEvent>(100);

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

    // 3. Publish Sport Mode Command (Mimic `sport_mode.py`)
    let request_payload = json!({
        "api_id": 1034, // SPORT_CMD GetState
        "id": 1
    });

    // We simulate the response arriving while publish_request_new is awaiting
    tokio::spawn({
        let env_clone = env.clone();
        async move {
            tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
            env_clone.push_incoming_message(
                r#"{
                "type": "response",
                "topic": "rt/api/sport/request",
                "data": {"header": {"identity": {"id": 1}}, "parameter": "mock_state"}
            }"#,
            );
        }
    });

    let pub_res: PortResult<serde_json::Value> = dc
        .publish_request_new("rt/api/sport/request", request_payload, Some(2.0))
        .await;
    assert!(
        pub_res.is_ok(),
        "Publish request should complete successfully"
    );

    // Check what was actually sent down the DataChannel
    let sent = env.dc_sent_messages.lock().unwrap();
    let last_sent = sent.last().expect("Should have sent a request");
    assert!(last_sent.contains("rt/api/sport/request"));
    assert!(last_sent.contains(r#""api_id":1034"#));
}

#[tokio::test]
async fn test_sport_mode_state_subscription_flow() {
    let (conn, dc, env, cb_rx) = setup_test_environment().await;

    // 1. Connect
    conn.connect().await.unwrap();

    // 2. Subscribe to LF_SPORT_MOD_STATE (Mimic `sport_mode_state.py`)
    let sub_res = dc.subscribe("rt/lf/sportmod/state");
    assert!(sub_res.is_ok());

    // Verify subscription message was sent
    {
        let sent = env.dc_sent_messages.lock().unwrap();
        let last_sent = sent.last().expect("Should have sent subscribe");
        assert!(last_sent.contains("rt/lf/sportmod/state"));
        assert!(last_sent.contains(r#""type":"subscribe""#));
    }

    // 3. Simulate incoming message for that topic from robot
    env.push_incoming_message(
        r#"{
        "type": "msg",
        "topic": "rt/lf/sportmod/state",
        "data": {"mode": "standing"}
    }"#,
    );

    // 4. Verify message is routed to Python's callback channel
    let evt = cb_rx.recv_timeout(std::time::Duration::from_secs(1));
    assert!(evt.is_ok(), "Should receive callback event");

    match evt.unwrap() {
        CallbackEvent::TopicCallback { topic, payload } => {
            assert_eq!(topic, "rt/lf/sportmod/state");
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

    // Verify correct data sent
    {
        let sent = env.dc_sent_messages.lock().unwrap();
        let last_sent = sent.last().expect("Should have sent video switch");
        assert!(last_sent.contains(r#""type":"vid""#));
        assert!(last_sent.contains(r#""data":"on""#));
    }

    // Disable Video Channel
    let switch_off_res = dc.switch_video_channel(false);
    assert!(switch_off_res.is_ok());

    // Verify correct data sent
    {
        let sent = env.dc_sent_messages.lock().unwrap();
        let last_sent = sent.last().expect("Should have sent video switch");
        assert!(last_sent.contains(r#""type":"vid""#));
        assert!(last_sent.contains(r#""data":"off""#));
    }

    // 3. Disconnect
    conn.disconnect().await;
    assert!(env.engine_closed.load(Ordering::Relaxed));
}

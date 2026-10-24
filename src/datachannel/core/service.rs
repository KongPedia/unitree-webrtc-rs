use super::chunks::ChunkAssembler;
use crate::datachannel::lidar::{LidarDecodeRequest, LidarMetadata, LidarWorkerPool};
use crate::diagnostics::RepeatLog;
use crate::infrastructure::security::encrypt_key;
use crate::protocol::constants::data_channel_type;
#[cfg(test)]
use crate::protocol::events::event_channel;
#[cfg(test)]
use crate::protocol::events::EventReceiver;
use crate::protocol::events::EventSender;
use crate::protocol::ingress::{parse_json, MAX_BINARY_BYTES, MAX_BINARY_EXPANSION_BYTES};
use crate::protocol::models::{CallbackEvent, DcMessage, IngressMessage, RequestIdentity};
use crate::protocol::ports::{DataChannelPort, PortResult};
use bytes::Bytes;
use chrono::Local;
use crossbeam_channel::{bounded, Receiver, RecvTimeoutError, Sender};
use serde_json::{json, Map, Value};
use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::oneshot;
use tracing::{debug, error, info, warn};

// Queue acceptance does not bound requests already sent while awaiting responses.
const MAX_PENDING_REQUESTS: usize = 128;

struct CoreDiagnostics {
    parse: RepeatLog,
    chunk: RepeatLog,
    callback: RepeatLog,
    binary: RepeatLog,
    lidar_submit: RepeatLog,
    lidar_invalid: RepeatLog,
    control_send: RepeatLog,
}

impl CoreDiagnostics {
    fn new() -> Self {
        Self {
            parse: RepeatLog::new("datachannel_json_parse_failed", "malformed_json"),
            chunk: RepeatLog::new("invalid_chunk_response", "invalid_chunk"),
            callback: RepeatLog::new("python_callback_drop", "queue_budget_or_stale"),
            binary: RepeatLog::new("binary_message_ignored", "malformed_frame"),
            lidar_submit: RepeatLog::new("lidar_worker_submit_failed", "queue_full_or_closed"),
            lidar_invalid: RepeatLog::new("invalid_lidar_response", "invalid_metadata_or_size"),
            control_send: RepeatLog::new("datachannel_control_send_failed", "enqueue_failed"),
        }
    }

    fn reset(&self) {
        for log in [
            &self.parse,
            &self.chunk,
            &self.callback,
            &self.binary,
            &self.lidar_submit,
            &self.lidar_invalid,
            &self.control_send,
        ] {
            log.reset();
        }
    }
}

struct PendingCallback {
    token: Arc<()>,
    sender: oneshot::Sender<PortResult<Value>>,
    expectation: ResponseExpectation,
    session_id: u64,
}

struct PendingRequestGuard<'a> {
    pending: &'a Mutex<HashMap<String, PendingCallback>>,
    chunks: &'a Mutex<ChunkAssembler>,
    key: String,
    token: Arc<()>,
}

impl Drop for PendingRequestGuard<'_> {
    fn drop(&mut self) {
        let mut pending = self.pending.lock().unwrap();
        if pending
            .get(&self.key)
            .is_some_and(|callback| Arc::ptr_eq(&callback.token, &self.token))
        {
            pending.remove(&self.key);
            self.chunks.lock().unwrap().remove(&self.key);
        }
    }
}

#[derive(Clone, Debug)]
struct ResponseExpectation {
    topics: Option<Vec<String>>,
    api_id: Option<i64>,
}

impl ResponseExpectation {
    fn any() -> Self {
        Self {
            topics: None,
            api_id: None,
        }
    }

    fn for_api_request(request_topic: &str, api_id: i64) -> Self {
        let response_topic = expected_response_topic(request_topic);
        let mut topics = vec![request_topic.to_string()];
        if response_topic != request_topic {
            topics.push(response_topic);
        }

        Self {
            topics: Some(topics),
            api_id: Some(api_id),
        }
    }

    fn validate(&self, message: &Value) -> PortResult<()> {
        if let Some(expected_topics) = &self.topics {
            let actual_topic = message
                .get("topic")
                .and_then(Value::as_str)
                .unwrap_or_default();
            if !expected_topics.iter().any(|topic| topic == actual_topic) {
                return Err("Response topic mismatch".into());
            }
        }

        if let Some(expected_api_id) = self.api_id {
            let actual_api_id = message
                .get("data")
                .and_then(|value| value.get("header"))
                .and_then(|value| value.get("identity"))
                .and_then(|value| value.get("api_id"))
                .and_then(Value::as_i64);
            if actual_api_id != Some(expected_api_id) {
                return Err(format!(
                    "Response api_id mismatch: expected {expected_api_id}, got {:?}",
                    actual_api_id
                ));
            }
        }

        Ok(())
    }
}

fn expected_response_topic(request_topic: &str) -> String {
    request_topic
        .strip_suffix("/request")
        .map(|prefix| format!("{prefix}/response"))
        .unwrap_or_else(|| request_topic.to_string())
}

struct DataChannelTypes {
    validation: &'static str,
    subscribe: &'static str,
    unsubscribe: &'static str,
    msg: &'static str,
    request: &'static str,
    response: &'static str,
    vid: &'static str,
    aud: &'static str,
    err: &'static str,
    heartbeat: &'static str,
    rtc_inner_req: &'static str,
    rtc_report: &'static str,
    add_error: &'static str,
    rm_error: &'static str,
    errors: &'static str,
}

fn dc_types() -> &'static DataChannelTypes {
    static TYPES: OnceLock<DataChannelTypes> = OnceLock::new();
    TYPES.get_or_init(|| {
        let map = data_channel_type();
        let get = |key: &'static str| {
            *map.get(key)
                .unwrap_or_else(|| panic!("Missing data channel type key: {key}"))
        };

        DataChannelTypes {
            validation: get("VALIDATION"),
            subscribe: get("SUBSCRIBE"),
            unsubscribe: get("UNSUBSCRIBE"),
            msg: get("MSG"),
            request: get("REQUEST"),
            response: get("RESPONSE"),
            vid: get("VID"),
            aud: get("AUD"),
            err: get("ERR"),
            heartbeat: get("HEARTBEAT"),
            rtc_inner_req: get("RTC_INNER_REQ"),
            rtc_report: get("RTC_REPORT"),
            add_error: get("ADD_ERROR"),
            rm_error: get("RM_ERROR"),
            errors: get("ERRORS"),
        }
    })
}

pub struct DataChannelService<T>
where
    T: DataChannelPort + 'static,
{
    transport: Arc<T>,
    pending_callbacks: Arc<Mutex<HashMap<String, PendingCallback>>>,
    subscriptions: Arc<Mutex<HashSet<String>>>,
    chunk_data_storage: Arc<Mutex<ChunkAssembler>>,
    // Public clones retain lifetime; worker clones only observe the weak token.
    external_owner: Option<Arc<()>>,
    owner_lifetime: Weak<()>,
    lifecycle_lock: Arc<Mutex<()>>,
    validation_key: Arc<Mutex<String>>,
    callback_events_tx: EventSender,
    data_channel_opened: Arc<AtomicBool>,
    heartbeat_last_response: Arc<AtomicU64>,
    heartbeat_running: Arc<AtomicBool>,
    heartbeat_stop_tx: Arc<Mutex<Option<Sender<()>>>>,
    network_probe_running: Arc<AtomicBool>,
    throttle_limits: Arc<HashMap<String, f64>>,
    last_process_time: Arc<Mutex<HashMap<String, f64>>>,
    decoder_type: Arc<Mutex<String>>,
    lidar_worker_pool: Arc<LidarWorkerPool>,
    is_remote_connection: bool,
    session_id: Arc<AtomicU64>,
    request_sequence: Arc<AtomicU64>,
    diagnostics: Arc<CoreDiagnostics>,
}

impl<T> Clone for DataChannelService<T>
where
    T: DataChannelPort + 'static,
{
    fn clone(&self) -> Self {
        Self {
            transport: Arc::clone(&self.transport),
            pending_callbacks: Arc::clone(&self.pending_callbacks),
            subscriptions: Arc::clone(&self.subscriptions),
            chunk_data_storage: Arc::clone(&self.chunk_data_storage),
            external_owner: self.external_owner.clone(),
            owner_lifetime: self.owner_lifetime.clone(),
            lifecycle_lock: Arc::clone(&self.lifecycle_lock),
            validation_key: Arc::clone(&self.validation_key),
            callback_events_tx: self.callback_events_tx.clone(),
            data_channel_opened: Arc::clone(&self.data_channel_opened),
            heartbeat_last_response: Arc::clone(&self.heartbeat_last_response),
            heartbeat_running: Arc::clone(&self.heartbeat_running),
            heartbeat_stop_tx: Arc::clone(&self.heartbeat_stop_tx),
            network_probe_running: Arc::clone(&self.network_probe_running),
            throttle_limits: Arc::clone(&self.throttle_limits),
            last_process_time: Arc::clone(&self.last_process_time),
            decoder_type: Arc::clone(&self.decoder_type),
            lidar_worker_pool: Arc::clone(&self.lidar_worker_pool),
            is_remote_connection: self.is_remote_connection,
            session_id: Arc::clone(&self.session_id),
            request_sequence: Arc::clone(&self.request_sequence),
            diagnostics: Arc::clone(&self.diagnostics),
        }
    }
}

impl<T> DataChannelService<T>
where
    T: DataChannelPort + 'static,
{
    /// Legacy constructor; prefer `try_new` for recoverable startup errors.
    ///
    /// # Panics
    /// Panics if the router thread cannot start.
    pub fn new(
        transport: Arc<T>,
        incoming_rx: Receiver<IngressMessage>,
        callback_events_tx: EventSender,
        lidar_worker_pool: Arc<LidarWorkerPool>,
        is_remote_connection: bool,
    ) -> Self {
        Self::try_new(
            transport,
            incoming_rx,
            callback_events_tx,
            lidar_worker_pool,
            is_remote_connection,
        )
        .expect("failed to spawn datachannel router thread")
    }

    /// Assemble routing, returning a sanitized error if its worker cannot start.
    ///
    /// # Errors
    /// Returns an error on OS thread creation failure; consumes and releases its inputs.
    pub fn try_new(
        transport: Arc<T>,
        incoming_rx: Receiver<IngressMessage>,
        callback_events_tx: EventSender,
        lidar_worker_pool: Arc<LidarWorkerPool>,
        is_remote_connection: bool,
    ) -> PortResult<Self> {
        Self::try_new_with_spawner(
            transport,
            incoming_rx,
            callback_events_tx,
            lidar_worker_pool,
            is_remote_connection,
            &mut |builder, job| builder.spawn(job),
        )
    }

    pub(crate) fn try_new_with_spawner<F>(
        transport: Arc<T>,
        incoming_rx: Receiver<IngressMessage>,
        callback_events_tx: EventSender,
        lidar_worker_pool: Arc<LidarWorkerPool>,
        is_remote_connection: bool,
        spawn: &mut F,
    ) -> PortResult<Self>
    where
        F: FnMut(
            thread::Builder,
            Box<dyn FnOnce() + Send>,
        ) -> std::io::Result<thread::JoinHandle<()>>,
    {
        let mut throttle_limits = HashMap::new();
        throttle_limits.insert("rt/utlidar/voxel_map_compressed".to_string(), 1.0 / 15.0);
        throttle_limits.insert(dc_types().vid.to_string(), 1.0 / 15.0);

        let external_owner = Arc::new(());
        let session_id = callback_events_tx.generation_state();
        session_id.store(Self::new_session_id(), Ordering::SeqCst);
        let service = Self {
            transport,
            pending_callbacks: Arc::new(Mutex::new(HashMap::new())),
            subscriptions: Arc::new(Mutex::new(HashSet::new())),
            chunk_data_storage: Arc::new(Mutex::new(ChunkAssembler::default())),
            external_owner: Some(external_owner.clone()),
            owner_lifetime: Arc::downgrade(&external_owner),
            lifecycle_lock: Arc::new(Mutex::new(())),
            validation_key: Arc::new(Mutex::new(String::new())),
            callback_events_tx,
            data_channel_opened: Arc::new(AtomicBool::new(false)),
            heartbeat_last_response: Arc::new(AtomicU64::new(0)),
            heartbeat_running: Arc::new(AtomicBool::new(false)),
            heartbeat_stop_tx: Arc::new(Mutex::new(None)),
            network_probe_running: Arc::new(AtomicBool::new(false)),
            throttle_limits: Arc::new(throttle_limits),
            last_process_time: Arc::new(Mutex::new(HashMap::new())),
            decoder_type: Arc::new(Mutex::new("native".to_string())),
            lidar_worker_pool,
            is_remote_connection,
            session_id,
            request_sequence: Arc::new(AtomicU64::new(Self::initial_request_sequence())),
            diagnostics: Arc::new(CoreDiagnostics::new()),
        };

        if service.prepare_ingress().is_err() {
            warn!(
                event = "ingress_generation_rejected",
                "Transport must accept generation before connecting"
            );
        }
        let owner = Arc::downgrade(service.external_owner.as_ref().unwrap());
        let mut service_for_router = service.clone();
        service_for_router.external_owner = None;
        spawn(
            thread::Builder::new().name("unitree-dc-router".to_string()),
            Box::new(move || {
                service_for_router.run_message_loop(incoming_rx, owner);
            }),
        )
        .map_err(|_| "Failed to start datachannel router".to_string())?;

        Ok(service)
    }

    pub async fn publish(
        &self,
        topic: &str,
        data: Option<Value>,
        msg_type: Option<&str>,
        timeout_secs: Option<f64>,
    ) -> PortResult<Value> {
        self.publish_with_expectation(
            topic,
            data,
            msg_type,
            timeout_secs,
            ResponseExpectation::any(),
        )
        .await
    }

    async fn publish_with_expectation(
        &self,
        topic: &str,
        data: Option<Value>,
        msg_type: Option<&str>,
        timeout_secs: Option<f64>,
        expectation: ResponseExpectation,
    ) -> PortResult<Value> {
        let timeout = timeout_secs.unwrap_or(10.0);
        let duration = validated_timeout(timeout)?;
        let effective_type = msg_type.unwrap_or(dc_types().msg);
        let key = Self::generate_message_key(
            effective_type,
            topic,
            data.as_ref().and_then(Self::outgoing_identifier_from_data),
        );

        let lifecycle = self.lifecycle_lock.lock().unwrap();
        if self.transport.ready_state() != "open" {
            return Err("Data channel is not open".to_string());
        }
        let (tx, rx) = oneshot::channel::<PortResult<Value>>();
        let token = Arc::new(());
        {
            let mut pending = self.pending_callbacks.lock().unwrap();
            if pending.contains_key(&key) {
                return Err(format!("Duplicate in-flight request key: {key}"));
            }
            if pending.len() >= MAX_PENDING_REQUESTS {
                return Err("DataChannel in-flight request limit reached".into());
            }
            pending.insert(
                key.clone(),
                PendingCallback {
                    token: Arc::clone(&token),
                    sender: tx,
                    expectation,
                    session_id: self.session_id.load(Ordering::Relaxed),
                },
            );
        }

        let _pending_guard = PendingRequestGuard {
            pending: &self.pending_callbacks,
            chunks: &self.chunk_data_storage,
            key: key.clone(),
            token,
        };
        self.send_message_locked(topic, data, effective_type)?;
        drop(lifecycle);

        match tokio::time::timeout(duration, rx).await {
            Ok(Ok(response)) => response,
            Ok(Err(_)) => Err("Response receiver dropped".to_string()),
            Err(_) => Err(format!(
                "Publish timeout after {timeout:.1}s for topic {topic}"
            )),
        }
    }

    pub fn publish_without_callback(
        &self,
        topic: &str,
        data: Option<Value>,
        msg_type: Option<&str>,
    ) -> PortResult<()> {
        let effective_type = msg_type.unwrap_or(dc_types().msg);
        self.send_message(topic, data, effective_type)
    }

    pub async fn publish_request_new(
        &self,
        topic: &str,
        options: Value,
        timeout_secs: Option<f64>,
    ) -> PortResult<Value> {
        let api_id = options
            .get("api_id")
            .and_then(Value::as_i64)
            .ok_or_else(|| "Please provide api_id".to_string())?;

        let id = options
            .get("id")
            .and_then(Value::as_i64)
            .unwrap_or_else(|| self.generated_identity_id());

        let mut request_payload = json!({
            "header": {
                "identity": {
                    "id": id,
                    "api_id": api_id,
                }
            },
            "parameter": "",
        });

        if let Some(parameter) = options.get("parameter") {
            if parameter.is_string() {
                request_payload["parameter"] = parameter.clone();
            } else {
                request_payload["parameter"] = Value::String(parameter.to_string());
            }
        }

        if options.get("priority").is_some() {
            request_payload["header"]["policy"] = json!({ "priority": 1 });
        }

        self.publish_with_expectation(
            topic,
            Some(request_payload),
            Some(dc_types().request),
            timeout_secs,
            ResponseExpectation::for_api_request(topic, api_id),
        )
        .await
    }

    pub fn subscribe(&self, topic: &str) -> PortResult<()> {
        let _lifecycle = self.lifecycle_lock.lock().unwrap();
        self.send_message_locked(topic, None, dc_types().subscribe)?;
        self.subscriptions.lock().unwrap().insert(topic.to_owned());
        Ok(())
    }

    pub fn unsubscribe(&self, topic: &str) -> PortResult<()> {
        let _lifecycle = self.lifecycle_lock.lock().unwrap();
        self.send_message_locked(topic, None, dc_types().unsubscribe)?;
        self.subscriptions.lock().unwrap().remove(topic);
        Ok(())
    }

    pub async fn disable_traffic_saving(&self, switch: bool) -> PortResult<bool> {
        let data = json!({
            "req_type": "disable_traffic_saving",
            "instruction": if switch { "on" } else { "off" },
        });

        let response = self
            .publish("", Some(data), Some(dc_types().rtc_inner_req), Some(10.0))
            .await?;

        let execution_ok = response
            .get("info")
            .and_then(|value| value.get("execution"))
            .and_then(Value::as_str)
            == Some("ok");

        if execution_ok {
            info!(
                event = "disable_traffic_saving",
                instruction = if switch { "on" } else { "off" },
                "Traffic saving instruction applied"
            );
        }

        Ok(execution_ok)
    }

    pub fn switch_video_channel(&self, switch: bool) -> PortResult<()> {
        self.publish_without_callback(
            "",
            Some(Value::String(if switch { "on" } else { "off" }.to_string())),
            Some(dc_types().vid),
        )?;
        info!(
            event = "video_channel_switch",
            value = if switch { "on" } else { "off" },
            "Video channel switch sent"
        );
        Ok(())
    }

    pub fn switch_audio_channel(&self, switch: bool) -> PortResult<()> {
        self.publish_without_callback(
            "",
            Some(Value::String(if switch { "on" } else { "off" }.to_string())),
            Some(dc_types().aud),
        )?;
        info!(
            event = "audio_channel_switch",
            value = if switch { "on" } else { "off" },
            "Audio channel switch sent"
        );
        Ok(())
    }

    pub async fn wait_datachannel_open(&self, timeout_secs: f64) -> PortResult<()> {
        let timeout = validated_timeout(timeout_secs)?;
        let started_at = std::time::Instant::now();

        loop {
            if self.is_open() {
                return Ok(());
            }

            if started_at.elapsed() > timeout {
                return Err("Data channel did not open in time".to_string());
            }

            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    pub fn ready_state(&self) -> &'static str {
        self.transport.ready_state()
    }

    pub fn is_open(&self) -> bool {
        self.transport.ready_state() == "open" && self.data_channel_opened.load(Ordering::Relaxed)
    }

    pub fn heartbeat_running(&self) -> bool {
        self.heartbeat_running.load(Ordering::Relaxed)
    }

    pub fn last_heartbeat_age_s(&self) -> Option<u64> {
        let last = self.heartbeat_last_response.load(Ordering::Relaxed);
        if last == 0 {
            return None;
        }
        Some(Self::now_secs_u64().saturating_sub(last))
    }

    pub fn should_process(&self, topic_or_type: &str) -> bool {
        if let Some(limit) = self.throttle_limits.get(topic_or_type) {
            let now = Self::now_secs_f64();
            let mut last_map = self.last_process_time.lock().unwrap();
            let last = last_map.get(topic_or_type).copied().unwrap_or(0.0);
            if now - last >= *limit {
                last_map.insert(topic_or_type.to_string(), now);
                return true;
            }
            return false;
        }

        true
    }

    fn stop_heartbeat(&self) {
        self.heartbeat_running.store(false, Ordering::Relaxed);
        // Disconnect persists even when the heartbeat has not entered its wait yet.
        self.heartbeat_stop_tx.lock().unwrap().take();
    }

    pub fn stop_background_tasks(&self) {
        self.stop_heartbeat();
        self.network_probe_running.store(false, Ordering::Relaxed);
        self.data_channel_opened.store(false, Ordering::Relaxed);
        self.start_new_session();
    }

    pub fn start_new_session(&self) {
        let _lifecycle = self.lifecycle_lock.lock().unwrap();
        self.diagnostics.reset();
        let generation = self
            .session_id
            .fetch_add(1, Ordering::SeqCst)
            .wrapping_add(1);
        if self.transport.set_ingress_generation(generation).is_err() {
            warn!(
                event = "ingress_generation_rejected",
                "Transport cannot prepare a new ingress session"
            );
        }
        self.stop_heartbeat();
        self.network_probe_running.store(false, Ordering::Relaxed);
        self.data_channel_opened.store(false, Ordering::Relaxed);
        self.heartbeat_last_response.store(0, Ordering::Relaxed);
        self.chunk_data_storage.lock().unwrap().clear();
        self.fail_all_pending("DataChannel session reset");
    }

    pub fn prepare_ingress(&self) -> PortResult<()> {
        let _lifecycle = self.lifecycle_lock.lock().unwrap();
        self.transport
            .set_ingress_generation(self.session_id.load(Ordering::SeqCst))
    }

    pub fn set_decoder(&self, decoder_type: &str) -> PortResult<()> {
        if decoder_type == "libvoxel" {
            return Err(
                "The 'libvoxel' decoder is not supported in the Rust backend. Use 'native'."
                    .to_string(),
            );
        }

        if decoder_type != "native" {
            return Err("Invalid decoder type. Choose 'native'.".to_string());
        }

        let mut current = self.decoder_type.lock().unwrap();
        *current = decoder_type.to_string();
        Ok(())
    }

    pub fn decoder_name(&self) -> String {
        let current = self.decoder_type.lock().unwrap();
        match current.as_str() {
            "native" => "NativeDecoder".to_string(),
            _ => "UnsupportedDecoder".to_string(),
        }
    }

    fn run_message_loop(&self, incoming_rx: Receiver<IngressMessage>, owner: Weak<()>) {
        while owner.upgrade().is_some() {
            if let Some(fault) = self.transport.take_transport_error() {
                self.process_ingress(fault);
            }
            self.expire_chunks();
            let message = match incoming_rx.recv_timeout(Duration::from_millis(100)) {
                Ok(message) => message,
                Err(RecvTimeoutError::Timeout) => continue,
                Err(RecvTimeoutError::Disconnected) => break,
            };
            self.process_ingress(message);
        }
        self.stop_background_tasks();
    }

    fn process_ingress(&self, message: IngressMessage) {
        let _lifecycle = self.lifecycle_lock.lock().unwrap();
        if message.generation != self.session_id.load(Ordering::SeqCst) {
            return;
        }
        match message.payload {
            DcMessage::TransportFault(error) => {
                let generation = self
                    .session_id
                    .fetch_add(1, Ordering::SeqCst)
                    .wrapping_add(1);
                if self.transport.set_ingress_generation(generation).is_err() {
                    self.diagnostics.control_send.record();
                }
                self.data_channel_opened.store(false, Ordering::Relaxed);
                self.stop_heartbeat();
                self.network_probe_running.store(false, Ordering::Relaxed);
                self.chunk_data_storage.lock().unwrap().clear();
                self.fail_all_pending(&error);
            }
            DcMessage::Text(text) => match parse_json(text.as_bytes()) {
                Ok(parsed) => {
                    self.run_resolve(&parsed);
                    self.handle_text_response(&parsed);
                    self.dispatch_subscription(parsed);
                }
                Err(_) => self.diagnostics.parse.record(),
            },
            DcMessage::Binary(binary) => {
                if let Some(parsed) = self.deal_array_buffer(&binary) {
                    self.run_resolve(&parsed);
                    self.dispatch_subscription(parsed);
                }
            }
        }
    }

    fn run_resolve(&self, message: &Value) {
        if message.get("type").is_none() {
            return;
        }

        if self.handle_chunked_response(message) {
            return;
        }

        self.resolve_pending_response(&Self::incoming_key(message), Cow::Borrowed(message));
    }

    fn handle_chunked_response(&self, message: &Value) -> bool {
        let (metadata, payload, path) = match (
            message.pointer("/data/content_info"),
            message.pointer("/info/file"),
        ) {
            (Some(info), _)
                if info.get("enable_chunking").and_then(Value::as_bool) == Some(true) =>
            {
                (info, message.pointer("/data/data"), "/data/data")
            }
            (_, Some(info))
                if info.get("enable_chunking").and_then(Value::as_bool) == Some(true) =>
            {
                (info, info.get("data"), "/info/file/data")
            }
            _ => return false,
        };
        let key = Self::incoming_key(message);
        let parse = || -> PortResult<Option<Vec<u8>>> {
            let index = metadata
                .get("chunk_index")
                .and_then(Value::as_u64)
                .and_then(|v| usize::try_from(v).ok())
                .ok_or("Invalid chunk index")?;
            let total = metadata
                .get("total_chunk_num")
                .and_then(Value::as_u64)
                .and_then(|v| usize::try_from(v).ok())
                .ok_or("Invalid chunk count")?;
            let bytes = payload
                .and_then(Self::value_to_chunk_bytes)
                .ok_or("Invalid chunk bytes")?;
            self.chunk_data_storage.lock().unwrap().insert(
                &key,
                path,
                index,
                total,
                bytes,
                std::time::Instant::now(),
            )
        };
        match parse() {
            Ok(Some(bytes)) => {
                if bytes.len() > MAX_BINARY_EXPANSION_BYTES {
                    self.fail_pending(&key, "Binary JSON expansion exceeds 1 MiB");
                    self.diagnostics.chunk.record();
                    return true;
                }
                let mut merged = message.clone();
                *merged
                    .pointer_mut(path)
                    .expect("validated chunk payload path") = Self::bytes_to_json_array(&bytes);
                self.resolve_pending_response(&key, Cow::Owned(merged));
            }
            Ok(None) => {}
            Err(error) => {
                self.chunk_data_storage.lock().unwrap().remove(&key);
                self.fail_pending(&key, &error);
                self.diagnostics.chunk.record();
            }
        }
        true
    }

    fn expire_chunks(&self) {
        let _lifecycle = self.lifecycle_lock.lock().unwrap();
        let expired = self
            .chunk_data_storage
            .lock()
            .unwrap()
            .expire(std::time::Instant::now());
        for key in expired {
            self.fail_pending(&key, "Chunk assembly deadline expired");
        }
    }

    fn fail_pending(&self, key: &str, error: &str) {
        let callback = self.pending_callbacks.lock().unwrap().remove(key);
        if let Some(callback) = callback {
            let _ = callback.sender.send(Err(error.to_owned()));
        }
    }

    fn handle_text_response(&self, message: &Value) {
        let msg_type = message
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let types = dc_types();

        match msg_type {
            value if value == types.validation => self.handle_validation(message),
            value if value == types.rtc_inner_req => self.handle_rtc_inner_req(message),
            value if value == types.heartbeat => {
                self.heartbeat_last_response
                    .store(Self::now_secs_u64(), Ordering::Relaxed);
            }
            value
                if value == types.errors || value == types.add_error || value == types.rm_error =>
            {
                self.handle_error_message(message)
            }
            value if value == types.err => self.handle_validation_err(message),
            value
                if value == types.response
                    || value == types.msg
                    || value == types.rtc_report
                    || value == types.subscribe
                    || value == types.unsubscribe
                    || value == types.vid
                    || value == types.aud => {}
            _ => {}
        }
    }

    fn handle_validation(&self, message: &Value) {
        if message.get("data").and_then(Value::as_str) == Some("Validation Ok.") {
            self.data_channel_opened.store(true, Ordering::Relaxed);
            self.heartbeat_last_response
                .store(Self::now_secs_u64(), Ordering::Relaxed);
            print_status("Data Channel", "verified");
            self.start_heartbeat_in_session();
            self.request_network_status_once();
            return;
        }

        if let Some(key) = message.get("data").and_then(Value::as_str) {
            {
                let mut validation_key = self.validation_key.lock().unwrap();
                *validation_key = key.to_string();
            }

            if let Ok(encrypted) = encrypt_key(key) {
                let _ = self.send_message_locked(
                    "",
                    Some(Value::String(encrypted)),
                    dc_types().validation,
                );
            }
        }
    }

    fn handle_validation_err(&self, message: &Value) {
        if message.get("info").and_then(Value::as_str) == Some("Validation Needed.") {
            let key = {
                let validation_key = self.validation_key.lock().unwrap();
                validation_key.clone()
            };

            if key.is_empty() {
                return;
            }

            if let Ok(encrypted) = encrypt_key(&key) {
                let _ = self.send_message_locked(
                    "",
                    Some(Value::String(encrypted)),
                    dc_types().validation,
                );
            }
        }
    }

    fn handle_rtc_inner_req(&self, message: &Value) {
        if let Some(info) = message.get("info") {
            if info.get("req_type").and_then(Value::as_str) == Some("rtt_probe_send_from_mechine") {
                let _ = self.send_message_locked("", Some(info.clone()), dc_types().rtc_inner_req);
            }

            if let Some(status) = info.get("status").and_then(Value::as_str) {
                match status {
                    "NetworkStatus.ON_4G_CONNECTED" => {
                        debug!(
                            event = "network_status",
                            status = status,
                            mode = "4G",
                            "Network status updated"
                        );
                        self.network_probe_running.store(false, Ordering::Relaxed);
                    }
                    "NetworkStatus.ON_WIFI_CONNECTED" => {
                        let mode = if self.is_remote_connection {
                            "STA-T"
                        } else {
                            "STA-L"
                        };
                        debug!(
                            event = "network_status",
                            status = status,
                            mode = mode,
                            "Network status updated"
                        );
                        self.network_probe_running.store(false, Ordering::Relaxed);
                    }
                    _ => {}
                }
            }
        }
    }

    fn handle_error_message(&self, message: &Value) {
        if !should_emit_error_log(message) {
            return;
        }
        let error_count = message.get("data").and_then(Value::as_array).map(Vec::len);
        error!(
            event = "go2_error",
            error_count, "Error message received from Go2"
        );
    }

    fn dispatch_subscription(&self, message: Value) {
        let topic = message
            .get("topic")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if topic.is_empty() {
            return;
        }

        if !self.should_process(topic) {
            return;
        }

        let should_dispatch = {
            let subscriptions = self.subscriptions.lock().unwrap();
            subscriptions.contains(topic)
        };

        if !should_dispatch {
            return;
        }

        let event = CallbackEvent::TopicCallback {
            topic: topic.to_string(),
            payload: message,
        };

        if self
            .callback_events_tx
            .for_generation(self.session_id.load(Ordering::SeqCst))
            .try_send(event)
            .is_err()
        {
            self.diagnostics.callback.record();
        }
    }

    fn send_message(&self, topic: &str, data: Option<Value>, msg_type: &str) -> PortResult<()> {
        let _lifecycle = self.lifecycle_lock.lock().unwrap();
        self.send_message_locked(topic, data, msg_type)
    }

    // Caller holds lifecycle_lock through readiness, registration and synchronous enqueue.
    fn send_message_locked(
        &self,
        topic: &str,
        data: Option<Value>,
        msg_type: &str,
    ) -> PortResult<()> {
        if self.transport.ready_state() != "open" {
            return Err("Data channel is not open".to_string());
        }
        let mut message = json!({
            "type": msg_type,
            "topic": topic,
        });

        if let Some(data) = data {
            message["data"] = data;
        }

        let text = serde_json::to_string(&message).map_err(|error| error.to_string())?;
        let result = self.transport.send_text(&text);
        if result.is_err() {
            self.diagnostics.control_send.record();
        }
        result
    }

    fn deal_array_buffer(&self, buffer: &[u8]) -> Option<Value> {
        if buffer.len() > MAX_BINARY_BYTES {
            self.diagnostics.binary.record();
            return None;
        }
        match super::framing::parse(buffer) {
            Ok((frame, header, payload)) => self.process_binary_payload(frame, header, payload),
            Err(_) => {
                self.diagnostics.binary.record();
                None
            }
        }
    }

    fn process_binary_payload(
        &self,
        frame: super::framing::BinaryFrame,
        mut decoded_json: Value,
        binary_data: &[u8],
    ) -> Option<Value> {
        let topic_or_type = decoded_json
            .get("topic")
            .and_then(Value::as_str)
            .or_else(|| decoded_json.get("type").and_then(Value::as_str))
            .unwrap_or_default()
            .to_owned();
        let data = decoded_json.get("data");
        let lidar_intent = frame == super::framing::BinaryFrame::Lidar
            || topic_or_type.starts_with("rt/utlidar/")
            || data.is_some_and(|data| {
                data.get("src_size").is_some()
                    || (data.get("origin").is_some() && data.get("resolution").is_some())
            });
        let metadata = data.and_then(LidarMetadata::from_json);
        if lidar_intent && metadata.is_none() {
            self.reject_binary_response(&decoded_json, "Invalid LiDAR metadata");
            return None;
        }
        if let Some(metadata) = metadata {
            if binary_data.len() > crate::datachannel::lidar::MAX_COMPRESSED_BYTES {
                self.reject_binary_response(
                    &decoded_json,
                    "LiDAR compressed size exceeds resource budget",
                );
                return None;
            }
            if self.decoder_type.lock().unwrap().as_str() != "native" {
                self.reject_binary_response(&decoded_json, "Unsupported LiDAR decoder");
                return None;
            }
            if !self.should_process(&topic_or_type) {
                return None;
            }
            let key = Self::incoming_key(&decoded_json);
            let request = LidarDecodeRequest {
                generation: self.session_id.load(Ordering::SeqCst),
                topic: topic_or_type.clone(),
                payload: decoded_json,
                compressed_data: Bytes::copy_from_slice(binary_data),
                metadata,
            };
            if let Err(error) = self.lidar_worker_pool.submit(request) {
                self.fail_pending(&key, &error);
                self.diagnostics.lidar_submit.record();
            }
            return None;
        }
        if binary_data.len() > MAX_BINARY_EXPANSION_BYTES {
            let key = Self::incoming_key(&decoded_json);
            self.chunk_data_storage.lock().unwrap().remove(&key);
            self.fail_pending(&key, "Binary JSON expansion exceeds 1 MiB");
            self.diagnostics.binary.record();
            return None;
        }
        if !self.should_process(&topic_or_type) {
            return None;
        }
        Self::inject_binary_data(&mut decoded_json, binary_data);
        Some(decoded_json)
    }

    fn reject_binary_response(&self, message: &Value, reason: &str) {
        let key = Self::incoming_key(message);
        self.chunk_data_storage.lock().unwrap().remove(&key);
        self.fail_pending(&key, reason);
        self.diagnostics.lidar_invalid.record();
    }

    fn inject_binary_data(decoded_json: &mut Value, binary_data: &[u8]) {
        if decoded_json
            .get("data")
            .and_then(Value::as_object)
            .is_none()
        {
            decoded_json["data"] = Value::Object(Map::new());
        }

        if let Some(data_obj) = decoded_json.get_mut("data").and_then(Value::as_object_mut) {
            data_obj.insert("data".to_string(), Self::bytes_to_json_array(binary_data));
        }
    }

    #[cfg(test)]
    fn start_heartbeat(&self) {
        let _lifecycle = self.lifecycle_lock.lock().unwrap();
        self.start_heartbeat_in_session();
    }

    // Caller owns lifecycle_lock across ingress handling or explicit test startup.
    fn start_heartbeat_in_session(&self) {
        if self
            .heartbeat_running
            .compare_exchange(false, true, Ordering::Relaxed, Ordering::Relaxed)
            .is_err()
        {
            return;
        }

        let (stop_tx, stop_rx) = bounded(0);
        *self.heartbeat_stop_tx.lock().unwrap() = Some(stop_tx);
        let epoch = self.session_id.load(Ordering::SeqCst);
        let owner = self.owner_lifetime.clone();
        let mut service = self.clone();
        service.external_owner = None;
        thread::Builder::new()
            .name("unitree-heartbeat".to_string())
            .spawn(move || {
                while service.heartbeat_running.load(Ordering::Relaxed)
                    && service.session_id.load(Ordering::SeqCst) == epoch
                    && owner.upgrade().is_some()
                {
                    let lifecycle = service.lifecycle_lock.lock().unwrap();
                    if service.session_id.load(Ordering::SeqCst) != epoch {
                        break;
                    }
                    let now = Self::now_secs_u64();
                    let last = service.heartbeat_last_response.load(Ordering::Relaxed);
                    if last > 0 && now.saturating_sub(last) > 6 {
                        warn!(
                            event = "heartbeat_timeout",
                            elapsed_sec = now.saturating_sub(last),
                            "Heartbeat timeout detected"
                        );
                        service.stop_heartbeat();
                        service.data_channel_opened.store(false, Ordering::Relaxed);
                        service.fail_all_pending("Heartbeat timeout");
                        service.chunk_data_storage.lock().unwrap().clear();
                        break;
                    }

                    if service.transport.ready_state() == "open" {
                        let formatted = Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
                        let data = json!({
                            "timeInStr": formatted,
                            "timeInNum": now,
                        });
                        let _ = service.send_message_locked("", Some(data), dc_types().heartbeat);
                    }

                    drop(lifecycle);
                    if stop_rx.recv_timeout(Duration::from_secs(2))
                        != Err(RecvTimeoutError::Timeout)
                    {
                        break;
                    }
                }
            })
            .expect("failed to spawn heartbeat thread");
    }

    fn request_network_status_once(&self) {
        if self
            .network_probe_running
            .compare_exchange(false, true, Ordering::Relaxed, Ordering::Relaxed)
            .is_err()
        {
            return;
        }

        let req_uuid = uuid::Uuid::new_v4().to_string();
        let data = json!({
            "req_type": "public_network_status",
            "uuid": req_uuid,
        });

        let _ = self.send_message_locked("", Some(data), dc_types().rtc_inner_req);
    }

    fn incoming_key(message: &Value) -> String {
        let msg_type = message
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let topic = message
            .get("topic")
            .and_then(Value::as_str)
            .unwrap_or_default();

        let identifier = message
            .get("data")
            .and_then(RequestIdentity::extract_key)
            .or_else(|| Self::nested_string(message, &["info", "uuid"]).map(ToString::to_string))
            .or_else(|| {
                Self::nested_string(message, &["info", "req_uuid"]).map(ToString::to_string)
            });

        Self::generate_message_key(msg_type, topic, identifier)
    }

    fn outgoing_identifier_from_data(data: &Value) -> Option<String> {
        RequestIdentity::extract_key(data)
    }

    fn generate_message_key(msg_type: &str, topic: &str, identifier: Option<String>) -> String {
        identifier.unwrap_or_else(|| format!("{msg_type} $ {topic}"))
    }

    fn nested_string<'a>(root: &'a Value, keys: &[&str]) -> Option<&'a str> {
        let mut current = root;
        for key in keys {
            current = current.get(*key)?;
        }
        current.as_str()
    }

    fn value_to_chunk_bytes(value: &Value) -> Option<Vec<u8>> {
        if let Some(text) = value.as_str() {
            return (text.len() <= super::chunks::MAX_TRANSFER_BYTES)
                .then(|| text.as_bytes().to_vec());
        }
        if value.as_array()?.len() > super::chunks::MAX_TRANSFER_BYTES {
            return None;
        }

        value
            .as_array()?
            .iter()
            .map(|element| u8::try_from(element.as_u64()?).ok())
            .collect()
    }

    fn bytes_to_json_array(bytes: &[u8]) -> Value {
        Value::Array(
            bytes
                .iter()
                .map(|byte| Value::Number(serde_json::Number::from(*byte)))
                .collect::<Vec<_>>(),
        )
    }

    #[cfg(test)]
    fn drop_pending_callbacks(&self, key: &str) {
        self.pending_callbacks.lock().unwrap().remove(key);
        self.chunk_data_storage.lock().unwrap().remove(key);
    }

    fn resolve_pending_response(&self, key: &str, message: Cow<'_, Value>) {
        let callback = {
            let mut pending = self.pending_callbacks.lock().unwrap();
            pending.remove(key)
        };

        let Some(callback) = callback else {
            return;
        };

        let current_session = self.session_id.load(Ordering::Relaxed);
        let result = if callback.session_id != current_session {
            Err("Response belongs to a stale DataChannel session".to_string())
        } else {
            callback
                .expectation
                .validate(&message)
                .map(|_| message.into_owned())
        };

        let _ = callback.sender.send(result);
    }

    fn fail_all_pending(&self, reason: &str) {
        let callbacks = {
            let mut pending = self.pending_callbacks.lock().unwrap();
            pending
                .drain()
                .map(|(_, callback)| callback)
                .collect::<Vec<_>>()
        };

        for callback in callbacks {
            let _ = callback.sender.send(Err(reason.to_string()));
        }
    }

    fn generated_identity_id(&self) -> i64 {
        const MAX_WIRE_ID: u64 = i32::MAX as u64;
        let next = self.request_sequence.fetch_add(1, Ordering::Relaxed);
        ((next % MAX_WIRE_ID) + 1) as i64
    }

    fn initial_request_sequence() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_micros() as u64)
            .unwrap_or(1)
    }

    fn new_session_id() -> u64 {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_nanos() as u64)
            .unwrap_or(0);
        timestamp ^ rand::random::<u64>()
    }

    fn now_secs_u64() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_secs())
            .unwrap_or(0)
    }

    fn now_secs_f64() -> f64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_secs_f64())
            .unwrap_or(0.0)
    }
}

fn print_status(status_type: &str, status_message: &str) {
    info!(
        event = "datachannel_status",
        component = status_type,
        state = status_message,
        "DataChannel state update"
    );
}

fn should_emit_error_log(message: &Value) -> bool {
    match message.get("data") {
        Some(Value::Array(items)) => !items.is_empty(),
        Some(Value::Null) | None => false,
        _ => true,
    }
}

pub(crate) fn validated_timeout(seconds: f64) -> PortResult<Duration> {
    let invalid = || {
        "Invalid timeout: expected finite nonnegative seconds representable as a deadline"
            .to_owned()
    };
    let duration = Duration::try_from_secs_f64(seconds).map_err(|_| invalid())?;
    std::time::Instant::now()
        .checked_add(duration)
        .ok_or_else(invalid)?;
    Ok(duration)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::models::{CallbackEvent, DcMessage};
    use crate::protocol::ports::{DataChannelPort, PortResult};
    use crossbeam_channel::bounded;
    use serde_json::{json, Value};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    #[test]
    fn boundary_response_topic_mismatch_redacts_arbitrary_peer_values() {
        let expectation = ResponseExpectation::for_api_request("expected/request", 1001);
        let error = expectation
            .validate(&json!({"topic":"sentinel-private-key"}))
            .unwrap_err();
        assert_eq!(error, "Response topic mismatch");
    }

    #[test]
    fn boundary_custom_ingress_rejects_oversized_text_before_validation_side_effects() {
        let (service, _channel, _tx, _rx) = make_service("open");
        let text = format!(
            "{{\"type\":\"validation\",\"data\":\"Validation Ok.\",\"padding\":\"{}\"}}",
            "x".repeat(1024 * 1024)
        );
        service.process_ingress(
            DcMessage::Text(text).for_generation(service.session_id.load(Ordering::SeqCst)),
        );
        assert!(
            !service.is_open(),
            "oversized custom ingress changed validation state"
        );
    }

    #[test]
    fn boundary_custom_ingress_rejects_excessive_json_nodes_before_side_effects() {
        let (service, _channel, _tx, _rx) = make_service("open");
        let text = format!(
            "{{\"type\":\"validation\",\"data\":\"Validation Ok.\",\"padding\":[{}]}}",
            std::iter::repeat_n("0", 65_536)
                .collect::<Vec<_>>()
                .join(",")
        );
        service.process_ingress(
            DcMessage::Text(text).for_generation(service.session_id.load(Ordering::SeqCst)),
        );
        assert!(
            !service.is_open(),
            "excessive JSON nodes changed validation state"
        );
    }

    #[test]
    fn boundary_binary_expansion_rejects_one_over_limit_without_partial_result() {
        let (service, _channel, _tx, _rx) = make_service("open");
        let header = br#"{"type":"res","topic":"custom"}"#;
        let mut frame = (header.len() as u16).to_le_bytes().to_vec();
        frame.extend_from_slice(&[0, 0]);
        frame.extend_from_slice(header);
        frame.resize(frame.len() + 1024 * 1024 + 1, 7);
        assert!(
            service.deal_array_buffer(&frame).is_none(),
            "oversized binary result expanded into JSON"
        );
    }

    fn expansion_chunk(file_schema: bool, index: usize, bytes: String) -> Value {
        let metadata = json!({"enable_chunking":true,"chunk_index":index,"total_chunk_num":2});
        if file_schema {
            json!({"type":"res","topic":"custom","info":{"file":{
                "enable_chunking":true,"chunk_index":index,"total_chunk_num":2,"data":bytes
            }}})
        } else {
            json!({"type":"res","topic":"custom","data":{"content_info":metadata,"data":bytes}})
        }
    }

    #[test]
    fn boundary_chunk_expansion_fails_pending_and_clears_assembly() {
        for file_schema in [false, true] {
            let (service, _channel, _tx, _rx) = make_service("open");
            let first = expansion_chunk(file_schema, 1, "a".repeat(512 * 1024));
            let key = DataChannelService::<MockDataChannel>::incoming_key(&first);
            let (sender, mut receiver) = oneshot::channel();
            let generation = service.session_id.load(Ordering::SeqCst);
            service.pending_callbacks.lock().unwrap().insert(
                key,
                PendingCallback {
                    token: Arc::new(()),
                    sender,
                    expectation: ResponseExpectation::any(),
                    session_id: generation,
                },
            );
            service.process_ingress(DcMessage::Text(first.to_string()).for_generation(generation));
            assert!(matches!(
                receiver.try_recv(),
                Err(oneshot::error::TryRecvError::Empty)
            ));
            let last = expansion_chunk(file_schema, 2, "b".repeat(512 * 1024 + 1));
            service.process_ingress(DcMessage::Text(last.to_string()).for_generation(generation));
            assert_eq!(
                receiver.try_recv().unwrap().err().as_deref(),
                Some("Binary JSON expansion exceeds 1 MiB")
            );
            assert!(service.chunk_data_storage.lock().unwrap().is_empty());
            assert!(service.pending_callbacks.lock().unwrap().is_empty());
        }
    }

    #[test]
    fn boundary_chunk_expansion_accepts_exact_limit_in_both_schemas() {
        for file_schema in [false, true] {
            let (service, _channel, _tx, _rx) = make_service("open");
            let first = expansion_chunk(file_schema, 1, "a".repeat(512 * 1024));
            let key = DataChannelService::<MockDataChannel>::incoming_key(&first);
            let (sender, mut receiver) = oneshot::channel();
            let generation = service.session_id.load(Ordering::SeqCst);
            service.pending_callbacks.lock().unwrap().insert(
                key,
                PendingCallback {
                    token: Arc::new(()),
                    sender,
                    expectation: ResponseExpectation::any(),
                    session_id: generation,
                },
            );
            service.process_ingress(DcMessage::Text(first.to_string()).for_generation(generation));
            assert!(matches!(
                receiver.try_recv(),
                Err(oneshot::error::TryRecvError::Empty)
            ));
            let last = expansion_chunk(file_schema, 2, "b".repeat(512 * 1024));
            service.process_ingress(DcMessage::Text(last.to_string()).for_generation(generation));
            let result = receiver.try_recv().unwrap().unwrap();
            let path = if file_schema {
                "/info/file/data"
            } else {
                "/data/data"
            };
            let bytes = result.pointer(path).unwrap().as_array().unwrap();
            assert_eq!(bytes.len(), 1024 * 1024);
            assert_eq!(bytes[0], 97);
            assert_eq!(bytes[bytes.len() - 1], 98);
            assert!(service.chunk_data_storage.lock().unwrap().is_empty());
        }
    }

    #[test]
    fn boundary_binary_expansion_accepts_exact_limit() {
        let (service, _channel, _tx, _rx) = make_service("open");
        let header = br#"{"type":"res","topic":"custom"}"#;
        let mut frame = (header.len() as u16).to_le_bytes().to_vec();
        frame.extend_from_slice(&[0, 0]);
        frame.extend_from_slice(header);
        frame.resize(frame.len() + 1024 * 1024, 7);
        let result = service.deal_array_buffer(&frame).unwrap();
        let bytes = result["data"]["data"].as_array().unwrap();
        assert_eq!(bytes.len(), 1024 * 1024);
        assert_eq!(bytes[0], 7);
        assert_eq!(bytes[bytes.len() - 1], 7);
    }

    #[test]
    fn boundary_binary_expansion_error_clears_pending_and_partial_chunks_before_throttle() {
        for topic in ["custom", "vid"] {
            let (service, _channel, _tx, _rx) = make_service("open");
            let header = json!({"type":"res","topic":topic});
            let key = DataChannelService::<MockDataChannel>::incoming_key(&header);
            let (sender, mut receiver) = oneshot::channel();
            service.pending_callbacks.lock().unwrap().insert(
                key.clone(),
                PendingCallback {
                    token: Arc::new(()),
                    sender,
                    expectation: ResponseExpectation::any(),
                    session_id: service.session_id.load(Ordering::SeqCst),
                },
            );
            service
                .chunk_data_storage
                .lock()
                .unwrap()
                .insert(&key, "/data/data", 1, 2, vec![1], std::time::Instant::now())
                .unwrap();
            service.last_process_time.lock().unwrap().insert(
                topic.into(),
                DataChannelService::<MockDataChannel>::now_secs_f64() + 1000.0,
            );
            let header = serde_json::to_vec(&header).unwrap();
            let mut frame = (header.len() as u16).to_le_bytes().to_vec();
            frame.extend_from_slice(&[0, 0]);
            frame.extend_from_slice(&header);
            frame.resize(frame.len() + 1024 * 1024 + 1, 7);
            assert!(service.deal_array_buffer(&frame).is_none());
            assert_eq!(
                receiver
                    .try_recv()
                    .expect("oversize failure must complete pending RPC immediately")
                    .err()
                    .as_deref(),
                Some("Binary JSON expansion exceeds 1 MiB")
            );
            assert!(service.chunk_data_storage.lock().unwrap().is_empty());
        }
    }

    #[derive(Default)]
    struct MockDataChannel {
        state: &'static str,
        sent: Arc<Mutex<Vec<String>>>,
        reject_send: AtomicBool,
        send_barriers: Mutex<Option<(Arc<std::sync::Barrier>, Arc<std::sync::Barrier>)>>,
    }

    impl DataChannelPort for MockDataChannel {
        fn send_text(&self, message: &str) -> PortResult<()> {
            if self.reject_send.load(Ordering::SeqCst) {
                return Err("Rejected send".into());
            }
            if let Some((entered, release)) = self.send_barriers.lock().unwrap().take() {
                entered.wait();
                release.wait();
            }
            self.sent.lock().unwrap().push(message.to_string());
            Ok(())
        }

        fn send_binary(&self, _bytes: &[u8]) -> PortResult<()> {
            Ok(())
        }

        fn set_message_sender(
            &self,
            _sender: crossbeam_channel::Sender<IngressMessage>,
        ) -> PortResult<()> {
            Ok(())
        }

        fn set_ingress_generation(&self, _generation: u64) -> PortResult<()> {
            Ok(())
        }

        fn ready_state(&self) -> &'static str {
            self.state
        }
    }

    #[tokio::test]
    async fn pending_request_admission_is_bounded_before_sending() {
        let (service, channel, _tx, _rx) = make_service("open");
        for index in 0..128 {
            let (sender, _) = oneshot::channel();
            service.pending_callbacks.lock().unwrap().insert(
                format!("fixture-{index}"),
                PendingCallback {
                    token: Arc::new(()),
                    sender,
                    expectation: ResponseExpectation::any(),
                    session_id: service.session_id.load(Ordering::SeqCst),
                },
            );
        }
        let error = service
            .publish("bounded", None, None, Some(0.0))
            .await
            .unwrap_err();
        assert!(error.contains("in-flight request limit"), "{error}");
        assert!(channel.sent.lock().unwrap().is_empty());
        assert_eq!(service.pending_callbacks.lock().unwrap().len(), 128);
        service.start_new_session();
        assert!(service.pending_callbacks.lock().unwrap().is_empty());
        // The reset releases admission, even though this fixture has no reply producer.
        assert!(service
            .publish("bounded", None, None, Some(0.0))
            .await
            .unwrap_err()
            .contains("timeout"));
        assert_eq!(channel.sent.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn transport_fault_fails_pending_without_response_timeout() {
        let (service, _channel, _tx, _rx) = make_service("open");
        let (sender, receiver) = oneshot::channel();
        let generation = service.session_id.load(Ordering::SeqCst);
        service.pending_callbacks.lock().unwrap().insert(
            "pending".into(),
            PendingCallback {
                token: Arc::new(()),
                sender,
                expectation: ResponseExpectation::any(),
                session_id: generation,
            },
        );
        service.process_ingress(
            DcMessage::TransportFault("DataChannel send failed".into()).for_generation(generation),
        );
        assert_eq!(
            receiver.await.unwrap().unwrap_err(),
            "DataChannel send failed"
        );
        assert!(service.pending_callbacks.lock().unwrap().is_empty());
        assert!(!service.is_open());
    }

    #[test]
    fn pending_registration_and_enqueue_finish_before_reset() {
        use std::sync::Barrier;
        let (service, channel, _tx, _rx) = make_service("open");
        let entered = Arc::new(Barrier::new(2));
        let release = Arc::new(Barrier::new(2));
        *channel.send_barriers.lock().unwrap() = Some((entered.clone(), release.clone()));
        let publisher = service.clone();
        let publish = thread::spawn(move || {
            tokio::runtime::Builder::new_current_thread()
                .enable_time()
                .build()
                .unwrap()
                .block_on(publisher.publish("topic", None, None, Some(1.0)))
        });
        entered.wait();
        assert_eq!(service.pending_callbacks.lock().unwrap().len(), 1);
        let resetting = service.clone();
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        let reset = thread::spawn(move || {
            resetting.start_new_session();
            done_tx.send(()).unwrap();
        });
        let reset_was_blocked = done_rx.recv_timeout(Duration::from_millis(50)).is_err();
        release.wait();
        reset.join().unwrap();
        assert!(publish
            .join()
            .unwrap()
            .unwrap_err()
            .contains("session reset"));
        assert!(reset_was_blocked);
        assert_eq!(channel.sent.lock().unwrap().len(), 1);
        assert!(service.pending_callbacks.lock().unwrap().is_empty());
    }

    #[test]
    fn reset_cannot_pass_an_outbound_admission_in_progress() {
        use std::sync::Barrier;
        let (service, channel, _tx, _rx) = make_service("open");
        let entered = Arc::new(Barrier::new(2));
        let release = Arc::new(Barrier::new(2));
        *channel.send_barriers.lock().unwrap() = Some((entered.clone(), release.clone()));
        let publisher = service.clone();
        let publish =
            thread::spawn(move || publisher.publish_without_callback("topic", None, None));
        entered.wait();
        let resetting = service.clone();
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        let reset = thread::spawn(move || {
            resetting.start_new_session();
            done_tx.send(()).unwrap();
        });
        let reset_was_blocked = done_rx.recv_timeout(Duration::from_millis(50)).is_err();
        release.wait();
        publish.join().unwrap().unwrap();
        reset.join().unwrap();
        assert!(
            reset_was_blocked,
            "reset passed an admitted send before synchronous enqueue completed"
        );
    }

    #[tokio::test]
    async fn invalid_timeouts_are_rejected_before_send() {
        for seconds in [f64::NAN, f64::INFINITY, -1.0, f64::MAX, 1e19] {
            let (service, channel, _tx, _rx) = make_service("open");
            assert!(service
                .publish("topic", None, None, Some(seconds))
                .await
                .is_err());
            assert!(channel.sent.lock().unwrap().is_empty());
            assert!(service.pending_callbacks.lock().unwrap().is_empty());
            assert!(service.wait_datachannel_open(seconds).await.is_err());
        }
    }

    #[test]
    fn chunk_bytes_reject_out_of_range_values() {
        assert_eq!(
            DataChannelService::<MockDataChannel>::value_to_chunk_bytes(&json!([0, 255])),
            Some(vec![0, 255])
        );
        assert!(
            DataChannelService::<MockDataChannel>::value_to_chunk_bytes(&json!([256])).is_none()
        );
        assert!(
            DataChannelService::<MockDataChannel>::value_to_chunk_bytes(&json!([-1])).is_none()
        );
    }

    #[test]
    fn chunk_final_index_does_not_complete_missing_chunks() {
        let (service, _channel, _tx, _rx) = make_service("open");
        let message = json!({"type":"res", "topic":"topic", "data":{"content_info":{"enable_chunking":true,"chunk_index":2,"total_chunk_num":2},"data":[2]}});
        assert!(service.handle_chunked_response(&message));
        assert!(!service.chunk_data_storage.lock().unwrap().is_empty());
        service.start_new_session();
        assert!(service.chunk_data_storage.lock().unwrap().is_empty());
    }

    #[test]
    fn router_releases_transport_when_external_owners_drop() {
        let (service, channel, _tx, _rx) = make_service("open");
        let weak = Arc::downgrade(&channel);
        drop(channel);
        drop(service);
        let deadline = std::time::Instant::now() + Duration::from_secs(1);
        while weak.upgrade().is_some() && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(
            weak.upgrade().is_none(),
            "router must not own the service lifetime"
        );
    }

    #[test]
    fn heartbeat_stop_releases_producers_before_next_tick() {
        let (service, channel, _ingress, events) = make_service("open");
        let entered = Arc::new(std::sync::Barrier::new(2));
        let release = Arc::new(std::sync::Barrier::new(2));
        *channel.send_barriers.lock().unwrap() = Some((entered.clone(), release.clone()));
        service.start_heartbeat();
        entered.wait();
        release.wait();
        // Stop takes the lifecycle lock after the admitted first heartbeat send.
        service.stop_background_tasks();
        let weak_transport = Arc::downgrade(&channel);
        drop(channel);
        drop(service);

        assert!(
            matches!(
                events.recv_timeout(Duration::from_secs(1)),
                Err(RecvTimeoutError::Disconnected)
            ),
            "stopped heartbeat retained event producers until its next live tick"
        );
        assert!(weak_transport.upgrade().is_none());
    }

    #[test]
    fn heartbeat_stop_before_wait_releases_producers_before_next_tick() {
        let (service, channel, _ingress, events) = make_service("open");
        let entered = Arc::new(std::sync::Barrier::new(2));
        let release = Arc::new(std::sync::Barrier::new(2));
        *channel.send_barriers.lock().unwrap() = Some((entered.clone(), release.clone()));
        service.start_heartbeat();
        entered.wait();
        let stopping = service.clone();
        let stop = thread::spawn(move || stopping.stop_background_tasks());
        let deadline = std::time::Instant::now() + Duration::from_secs(1);
        // Hold the first send until cancellation, so the worker cannot have parked yet.
        while service.heartbeat_stop_tx.lock().unwrap().is_some()
            && std::time::Instant::now() < deadline
        {
            thread::yield_now();
        }
        let cancelled_before_wait = service.heartbeat_stop_tx.lock().unwrap().is_none();
        release.wait();
        stop.join().unwrap();
        let weak_transport = Arc::downgrade(&channel);
        drop(channel);
        drop(service);

        assert!(
            cancelled_before_wait,
            "stop waited for the next heartbeat tick"
        );
        assert!(
            matches!(
                events.recv_timeout(Duration::from_secs(1)),
                Err(RecvTimeoutError::Disconnected)
            ),
            "cancellation before parking failed to release heartbeat producers"
        );
        assert!(weak_transport.upgrade().is_none());
    }

    #[test]
    fn heartbeat_final_owner_drop_releases_producers_before_next_tick() {
        let (service, channel, _ingress, events) = make_service("open");
        let entered = Arc::new(std::sync::Barrier::new(2));
        let release = Arc::new(std::sync::Barrier::new(2));
        *channel.send_barriers.lock().unwrap() = Some((entered.clone(), release.clone()));
        service.start_heartbeat();
        entered.wait();
        release.wait();
        let weak_transport = Arc::downgrade(&channel);
        drop(channel);
        drop(service);

        assert!(
            matches!(
                events.recv_timeout(Duration::from_secs(1)),
                Err(RecvTimeoutError::Disconnected)
            ),
            "orphaned heartbeat retained event producers until its next live tick"
        );
        assert!(weak_transport.upgrade().is_none());
    }

    fn heartbeat_without_router() -> (
        DataChannelService<MockDataChannel>,
        Arc<MockDataChannel>,
        EventReceiver,
    ) {
        let (_ingress, incoming) = bounded(1);
        let (events_tx, events) = event_channel(1).unwrap();
        let pool = Arc::new(LidarWorkerPool::new(events_tx.clone(), 1));
        let channel = Arc::new(MockDataChannel {
            state: "open",
            ..MockDataChannel::default()
        });
        let service = DataChannelService::try_new_with_spawner(
            channel.clone(),
            incoming,
            events_tx,
            pool,
            false,
            &mut |builder, router| {
                // Stub routing only: its final stop must not mask a missing heartbeat wake.
                drop(router);
                builder.spawn(|| {})
            },
        )
        .unwrap();
        let entered = Arc::new(std::sync::Barrier::new(2));
        let release = Arc::new(std::sync::Barrier::new(2));
        *channel.send_barriers.lock().unwrap() = Some((entered.clone(), release.clone()));
        service.start_heartbeat();
        entered.wait();
        release.wait();
        (service, channel, events)
    }

    #[test]
    fn heartbeat_reset_releases_producers_before_next_tick() {
        let (service, channel, events) = heartbeat_without_router();
        service.start_new_session();
        let weak_transport = Arc::downgrade(&channel);
        drop(channel);
        drop(service);

        assert!(
            matches!(
                events.recv_timeout(Duration::from_secs(1)),
                Err(RecvTimeoutError::Disconnected)
            ),
            "reset heartbeat retained event producers until its next live tick"
        );
        assert!(weak_transport.upgrade().is_none());
    }

    #[test]
    fn heartbeat_transport_fault_releases_producers_before_next_tick() {
        let (service, channel, events) = heartbeat_without_router();
        let generation = service.session_id.load(Ordering::SeqCst);
        service.process_ingress(
            DcMessage::TransportFault("fixture send failure".into()).for_generation(generation),
        );
        let weak_transport = Arc::downgrade(&channel);
        drop(channel);
        drop(service);

        assert!(
            matches!(
                events.recv_timeout(Duration::from_secs(1)),
                Err(RecvTimeoutError::Disconnected)
            ),
            "faulted heartbeat retained event producers until its next live tick"
        );
        assert!(weak_transport.upgrade().is_none());
    }

    #[test]
    fn heartbeat_reconnect_does_not_revive_old_worker() {
        let (service, channel, _tx, _rx) = make_service("open");
        service.start_heartbeat();
        let deadline = std::time::Instant::now() + Duration::from_secs(1);
        while channel.sent.lock().unwrap().is_empty() && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(channel.sent.lock().unwrap().len(), 1);
        service.start_new_session();
        service.start_heartbeat();
        let deadline = std::time::Instant::now() + Duration::from_secs(1);
        while channel.sent.lock().unwrap().len() < 2 && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(channel.sent.lock().unwrap().len(), 2);
        std::thread::sleep(Duration::from_millis(2200));
        assert_eq!(
            channel.sent.lock().unwrap().len(),
            3,
            "only the new epoch sends its next heartbeat"
        );
        service.stop_background_tasks();
    }

    #[tokio::test]
    async fn validated_flag_alone_does_not_make_closed_transport_ready() {
        let (service, _channel, _tx, _rx) = make_service("closed");
        service.data_channel_opened.store(true, Ordering::Relaxed);
        assert!(service.wait_datachannel_open(0.0).await.is_err());
    }

    #[test]
    fn both_chunk_schemas_reassemble_in_index_order() {
        for path in ["content", "file"] {
            let (service, _channel, _tx, _rx) = make_service("open");
            let mut message = json!({"type":"res", "topic":"topic"});
            if path == "content" {
                message["data"] = json!({"content_info":{"enable_chunking":true,"chunk_index":2,"total_chunk_num":2},"data":[2]});
            } else {
                message["info"] = json!({"file":{"enable_chunking":true,"chunk_index":2,"total_chunk_num":2,"data":[2]}});
            }
            let key = DataChannelService::<MockDataChannel>::incoming_key(&message);
            let (tx, mut rx) = oneshot::channel();
            service.pending_callbacks.lock().unwrap().insert(
                key,
                PendingCallback {
                    token: Arc::new(()),
                    sender: tx,
                    expectation: ResponseExpectation::any(),
                    session_id: service.session_id.load(Ordering::Relaxed),
                },
            );
            assert!(service.handle_chunked_response(&message));
            assert!(matches!(
                rx.try_recv(),
                Err(oneshot::error::TryRecvError::Empty)
            ));
            assert!(service.handle_chunked_response(&message));
            let (metadata_path, data_path) = if path == "content" {
                ("/data/content_info/chunk_index", "/data/data")
            } else {
                ("/info/file/chunk_index", "/info/file/data")
            };
            *message.pointer_mut(metadata_path).unwrap() = json!(1);
            *message.pointer_mut(data_path).unwrap() = json!([1]);
            assert!(service.handle_chunked_response(&message));
            assert_eq!(
                rx.try_recv().unwrap().unwrap().pointer(data_path).unwrap(),
                &json!([1, 2])
            );
        }
    }

    #[test]
    fn malformed_chunk_fails_pending_without_partial_success() {
        let (service, _channel, _tx, _rx) = make_service("open");
        let message = json!({"type":"res", "topic":"topic", "data":{"content_info":{"enable_chunking":true,"chunk_index":0,"total_chunk_num":2},"data":[256]}});
        let key = DataChannelService::<MockDataChannel>::incoming_key(&message);
        let (tx, mut rx) = oneshot::channel();
        service.pending_callbacks.lock().unwrap().insert(
            key,
            PendingCallback {
                token: Arc::new(()),
                sender: tx,
                expectation: ResponseExpectation::any(),
                session_id: service.session_id.load(Ordering::Relaxed),
            },
        );
        service.run_resolve(&message);
        assert!(rx.try_recv().unwrap().is_err());
        assert!(service.chunk_data_storage.lock().unwrap().is_empty());
    }

    #[test]
    fn request_cleanup_discards_partial_chunks() {
        let (service, _channel, _tx, _rx) = make_service("open");
        let message = json!({"type":"res", "topic":"topic", "data":{"content_info":{"enable_chunking":true,"chunk_index":1,"total_chunk_num":2},"data":[1]}});
        service.handle_chunked_response(&message);
        assert!(!service.chunk_data_storage.lock().unwrap().is_empty());
        service.drop_pending_callbacks(&DataChannelService::<MockDataChannel>::incoming_key(
            &message,
        ));
        assert!(service.chunk_data_storage.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn cancelled_publish_reclaims_pending_and_partial_chunks() {
        let (service, channel, _tx, _rx) = make_service("open");
        let requester = service.clone();
        let task =
            tokio::spawn(async move { requester.publish("topic", None, None, Some(3600.0)).await });
        while channel.sent.lock().unwrap().is_empty() {
            tokio::task::yield_now().await;
        }
        let message = json!({"type":"msg", "topic":"topic", "data":{"content_info":{"enable_chunking":true,"chunk_index":1,"total_chunk_num":2},"data":[1]}});
        service.handle_chunked_response(&message);
        assert!(!service.chunk_data_storage.lock().unwrap().is_empty());
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        assert!(service.pending_callbacks.lock().unwrap().is_empty());
        assert!(service.chunk_data_storage.lock().unwrap().is_empty());
    }

    #[test]
    fn invalid_lidar_metadata_never_falls_back_to_raw_binary() {
        for data in [
            json!({"origin":[0,0,0],"resolution":0.1,"src_size":usize::MAX}),
            json!({"origin":[0,0],"resolution":0.1,"src_size":1}),
            json!({"origin":[0,0,0],"resolution":0.0,"src_size":1}),
        ] {
            let (service, _channel, _tx, _rx) = make_service("open");
            let message = json!({"type":"msg", "topic":"rt/custom", "data":data});
            let json = serde_json::to_vec(&message).unwrap();
            let mut frame = (json.len() as u16).to_le_bytes().to_vec();
            frame.extend_from_slice(&[0, 0]);
            frame.extend_from_slice(&json);
            frame.push(1);
            assert!(service.deal_array_buffer(&frame).is_none());
        }
    }

    #[test]
    fn oversized_lidar_compressed_payload_rejects_before_worker_handoff() {
        let (service, _channel, _tx, _rx) = make_service("open");
        let message = json!({"type":"msg", "topic":"rt/custom", "data":{"origin":[0,0,0],"resolution":0.1,"src_size":1}});
        let key = DataChannelService::<MockDataChannel>::incoming_key(&message);
        let (tx, mut rx) = oneshot::channel();
        service.pending_callbacks.lock().unwrap().insert(
            key,
            PendingCallback {
                token: Arc::new(()),
                sender: tx,
                expectation: ResponseExpectation::any(),
                session_id: service.session_id.load(Ordering::Relaxed),
            },
        );
        let json = serde_json::to_vec(&message).unwrap();
        let mut frame = (json.len() as u16).to_le_bytes().to_vec();
        frame.extend_from_slice(&[0, 0]);
        frame.extend_from_slice(&json);
        frame.resize(
            frame.len() + crate::datachannel::lidar::MAX_COMPRESSED_BYTES + 1,
            0,
        );
        assert!(service.deal_array_buffer(&frame).is_none());
        assert!(rx.try_recv().unwrap().is_err());
    }

    #[test]
    fn cancelled_old_request_guard_preserves_replacement() {
        let (service, _channel, _tx, _rx) = make_service("open");
        let guard = PendingRequestGuard {
            pending: &service.pending_callbacks,
            chunks: &service.chunk_data_storage,
            key: "same-key".into(),
            token: Arc::new(()),
        };
        let (sender, _receiver) = oneshot::channel();
        service.pending_callbacks.lock().unwrap().insert(
            "same-key".into(),
            PendingCallback {
                token: Arc::new(()),
                sender,
                expectation: ResponseExpectation::any(),
                session_id: service.session_id.load(Ordering::Relaxed),
            },
        );
        service
            .chunk_data_storage
            .lock()
            .unwrap()
            .insert(
                "same-key",
                "content",
                1,
                2,
                vec![1],
                std::time::Instant::now(),
            )
            .unwrap();
        drop(guard);
        assert!(service
            .pending_callbacks
            .lock()
            .unwrap()
            .contains_key("same-key"));
        assert!(!service.chunk_data_storage.lock().unwrap().is_empty());
    }

    #[test]
    fn subscription_sets_commit_only_after_accepted_send() {
        let (service, channel, _tx, _rx) = make_service("open");
        channel.reject_send.store(true, Ordering::SeqCst);
        assert!(service.subscribe("topic").is_err());
        assert!(!service.subscriptions.lock().unwrap().contains("topic"));
        channel.reject_send.store(false, Ordering::SeqCst);
        service.subscribe("topic").unwrap();
        channel.reject_send.store(true, Ordering::SeqCst);
        assert!(service.unsubscribe("topic").is_err());
        assert!(service.subscriptions.lock().unwrap().contains("topic"));
    }

    #[test]
    fn stale_ingress_cannot_validate_or_retain_chunks() {
        let (service, _channel, _tx, _rx) = make_service("open");
        let old = service.session_id.load(Ordering::SeqCst);
        service.start_new_session();
        service.process_ingress(
            DcMessage::Text(json!({"type":"validation","data":"Validation Ok."}).to_string())
                .for_generation(old),
        );
        assert!(!service.data_channel_opened.load(Ordering::SeqCst));
        service.process_ingress(
            DcMessage::Text(json!({"type":"heartbeat"}).to_string()).for_generation(old),
        );
        assert_eq!(service.heartbeat_last_response.load(Ordering::SeqCst), 0);
        service.process_ingress(DcMessage::Text(json!({"type":"res", "topic":"topic", "data":{"content_info":{"enable_chunking":true,"chunk_index":1,"total_chunk_num":2},"data":[1]}}).to_string()).for_generation(old));
        assert!(service.chunk_data_storage.lock().unwrap().is_empty());
    }

    #[test]
    fn stale_response_cannot_resolve_reused_user_request_id() {
        let (service, _channel, _tx, _rx) = make_service("open");
        let old = service.session_id.load(Ordering::SeqCst);
        service.start_new_session();
        let response = json!({"type":"res","topic":"rt/api/sport/response","data":{"header":{"identity":{"id":42,"api_id":1001}}}});
        let key = DataChannelService::<MockDataChannel>::incoming_key(&response);
        let (sender, mut receiver) = oneshot::channel();
        service.pending_callbacks.lock().unwrap().insert(
            key,
            PendingCallback {
                token: Arc::new(()),
                sender,
                expectation: ResponseExpectation::for_api_request("rt/api/sport/request", 1001),
                session_id: service.session_id.load(Ordering::SeqCst),
            },
        );
        service.process_ingress(DcMessage::Text(response.to_string()).for_generation(old));
        assert!(matches!(
            receiver.try_recv(),
            Err(oneshot::error::TryRecvError::Empty)
        ));
        service.process_ingress(
            DcMessage::Text(response.to_string())
                .for_generation(service.session_id.load(Ordering::SeqCst)),
        );
        assert!(receiver.try_recv().unwrap().is_ok());
    }

    #[test]
    fn ingress_generation_is_checked_after_acquiring_processing_lock() {
        let (service, _channel, _tx, _rx) = make_service("open");
        let epoch = service.session_id.load(Ordering::SeqCst);
        let guard = service.lifecycle_lock.lock().unwrap();
        let worker = service.clone();
        let (started_tx, started_rx) = bounded(1);
        let (done_tx, done_rx) = bounded(1);
        let task = thread::spawn(move || {
            started_tx.send(()).unwrap();
            worker.process_ingress(
                DcMessage::Text(json!({"type":"validation","data":"Validation Ok."}).to_string())
                    .for_generation(epoch),
            );
            done_tx.send(()).unwrap();
        });
        started_rx.recv().unwrap();
        assert!(
            done_rx.recv_timeout(Duration::from_millis(50)).is_err(),
            "processing must wait for lifecycle lock"
        );
        // Model a reset after dequeue while processing is gated; no new envelope may be stamped with the old epoch.
        service.session_id.fetch_add(1, Ordering::SeqCst);
        drop(guard);
        done_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        task.join().unwrap();
        assert!(!service.data_channel_opened.load(Ordering::SeqCst));
    }

    #[test]
    fn malformed_binary_header_does_not_kill_router() {
        let (service, _channel, sender, events) = make_service("open");
        service.subscribe("topic").unwrap();
        let mut frame = vec![4, 0, 0, 0];
        frame.extend_from_slice(b"true");
        sender.send(DcMessage::Binary(frame.into())).unwrap();
        sender
            .send(DcMessage::Text(
                json!({"type":"msg","topic":"topic","data":1}).to_string(),
            ))
            .unwrap();
        let event = events.recv_timeout(Duration::from_secs(1)).unwrap();
        assert!(matches!(event, CallbackEvent::TopicCallback {payload,..} if payload["data"] == 1));
    }

    #[test]
    fn publish_request_builds_payload() {
        // Verify that publish_without_callback produces a correctly-shaped outgoing "msg"
        // frame over the DataChannel — this is the send path that publish_request shares.
        let (_dc_tx, dc_rx) = bounded::<IngressMessage>(32);
        let (callback_tx, _callback_rx) = event_channel(32).unwrap();
        let lidar_pool = crate::datachannel::lidar::create_worker_pool(callback_tx.clone());
        let channel = Arc::new(MockDataChannel {
            state: "open",
            sent: Arc::new(Mutex::new(Vec::new())),
            reject_send: AtomicBool::new(false),
            send_barriers: Mutex::new(None),
        });

        let service =
            DataChannelService::new(channel.clone(), dc_rx, callback_tx, lidar_pool, false);

        // publish_without_callback is the simplest form of the send path
        service
            .publish_without_callback("rt/api/sport/request", Some(json!({"api_id":1016})), None)
            .expect("should succeed");

        let sent = channel.sent.lock().unwrap();
        assert!(!sent.is_empty(), "expected at least one message");
        let frame: Value = serde_json::from_str(&sent[0]).unwrap();
        assert_eq!(frame["type"], dc_types().msg);
        assert_eq!(frame["topic"], "rt/api/sport/request");
    }

    #[test]
    fn error_log_is_suppressed_for_empty_error_list() {
        let message = json!({
            "type": "errors",
            "data": [],
        });

        assert!(!should_emit_error_log(&message));
    }

    #[test]
    fn error_log_is_emitted_for_non_empty_error_list() {
        let message = json!({
            "type": "errors",
            "data": [[1700000000, 100, 1]],
        });

        assert!(should_emit_error_log(&message));
    }

    #[test]
    fn peer_error_log_omits_raw_payload_and_preserves_subscription() {
        #[derive(Clone)]
        struct Capture(Arc<Mutex<Vec<u8>>>);
        impl std::io::Write for Capture {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                self.0.lock().unwrap().extend_from_slice(bytes);
                Ok(bytes.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let (service, _, _, callback_rx) = make_service("open");
        service.subscribe("errors").unwrap();
        let secret = "sentinel-private-key".repeat(1024);
        let message = json!({"type": "errors", "topic": "errors", "data": [secret]});
        let expected = message.clone();
        let bytes = Arc::new(Mutex::new(Vec::new()));
        let output = Arc::clone(&bytes);
        let subscriber = tracing_subscriber::fmt()
            .without_time()
            .with_ansi(false)
            .with_writer(move || Capture(Arc::clone(&output)))
            .finish();
        tracing::subscriber::with_default(subscriber, || {
            service.handle_error_message(&message);
            service.dispatch_subscription(message);
        });
        let log = String::from_utf8(bytes.lock().unwrap().clone()).unwrap();
        assert!(log.contains("ERROR"));
        assert!(log.contains("go2_error"));
        assert!(!log.contains("sentinel-private-key"));
        assert!(log.contains("error_count=1"));
        assert!(log.len() < 512);
        assert!(
            matches!(callback_rx.try_recv().unwrap(), CallbackEvent::TopicCallback { payload, .. } if payload == expected)
        );
    }

    // ─────────────────────────────────────────────────────────────────────────
    #[derive(Clone)]
    struct FixtureSender {
        sender: crossbeam_channel::Sender<IngressMessage>,
        generation: Arc<AtomicU64>,
    }

    impl FixtureSender {
        fn send(
            &self,
            message: DcMessage,
        ) -> Result<(), crossbeam_channel::SendError<IngressMessage>> {
            self.sender
                .send(message.for_generation(self.generation.load(Ordering::SeqCst)))
        }
    }

    // Helper: creates a DataChannelService with a MockDataChannel and returns
    // all channels needed for test assertions.
    // ─────────────────────────────────────────────────────────────────────────
    fn make_service(
        state: &'static str,
    ) -> (
        DataChannelService<MockDataChannel>,
        Arc<MockDataChannel>,
        FixtureSender,
        EventReceiver,
    ) {
        let (dc_tx, dc_rx) = bounded::<IngressMessage>(64);
        let (cb_tx, cb_rx) = event_channel(64).unwrap();
        let lidar_pool = crate::datachannel::lidar::create_worker_pool(cb_tx.clone());
        let channel = Arc::new(MockDataChannel {
            state,
            sent: Arc::new(Mutex::new(Vec::new())),
            reject_send: AtomicBool::new(false),
            send_barriers: Mutex::new(None),
        });
        let service =
            DataChannelService::new(Arc::clone(&channel), dc_rx, cb_tx, lidar_pool, false);
        let sender = FixtureSender {
            sender: dc_tx,
            generation: Arc::clone(&service.session_id),
        };
        (service, channel, sender, cb_rx)
    }

    #[test]
    fn subscription_moves_original_json_allocation_with_or_without_waiter() {
        for has_waiter in [false, true] {
            let (service, _channel, _ingress, events) = make_service("open");
            service.subscribe("topic").unwrap();
            let message = json!({"type":"res", "topic":"topic", "data":"owned-payload"});
            let original = message["data"].as_str().unwrap().as_ptr();
            let mut response = has_waiter.then(|| {
                let (sender, receiver) = oneshot::channel();
                service.pending_callbacks.lock().unwrap().insert(
                    DataChannelService::<MockDataChannel>::incoming_key(&message),
                    PendingCallback {
                        token: Arc::new(()),
                        sender,
                        expectation: ResponseExpectation::any(),
                        session_id: service.session_id.load(Ordering::Relaxed),
                    },
                );
                receiver
            });
            service.run_resolve(&message);
            let resolved = response.as_mut().map(|rx| rx.try_recv().unwrap().unwrap());
            if let Some(resolved) = &resolved {
                assert_eq!(resolved, &message);
                assert_ne!(resolved["data"].as_str().unwrap().as_ptr(), original);
            }
            service.dispatch_subscription(message);
            let CallbackEvent::TopicCallback { payload, .. } = events.try_recv().unwrap() else {
                panic!("expected topic callback");
            };
            assert_eq!(payload["data"], "owned-payload");
            assert_eq!(payload["data"].as_str().unwrap().as_ptr(), original);
        }
    }

    #[test]
    fn owned_pending_response_preserves_json_allocation() {
        let (service, _channel, _ingress, _events) = make_service("open");
        let message = json!({"type":"res", "topic":"topic", "data":"merged-payload"});
        let original = message["data"].as_str().unwrap().as_ptr();
        let key = DataChannelService::<MockDataChannel>::incoming_key(&message);
        let (sender, mut response) = oneshot::channel();
        service.pending_callbacks.lock().unwrap().insert(
            key.clone(),
            PendingCallback {
                token: Arc::new(()),
                sender,
                expectation: ResponseExpectation::any(),
                session_id: service.session_id.load(Ordering::Relaxed),
            },
        );
        service.resolve_pending_response(&key, Cow::Owned(message));
        let resolved = response.try_recv().unwrap().unwrap();
        assert_eq!(resolved["data"].as_str().unwrap().as_ptr(), original);
    }

    #[test]
    fn subscription_quota_failure_does_not_prevent_pending_completion() {
        let (service, _channel, _ingress, events) = make_service("open");
        service.subscribe("topic").unwrap();
        let message = json!({"type":"res", "topic":"topic", "data":"payload"});
        for _ in 0..64 {
            service.dispatch_subscription(message.clone());
        }
        let (sender, mut response) = oneshot::channel();
        service.pending_callbacks.lock().unwrap().insert(
            DataChannelService::<MockDataChannel>::incoming_key(&message),
            PendingCallback {
                token: Arc::new(()),
                sender,
                expectation: ResponseExpectation::any(),
                session_id: service.session_id.load(Ordering::Relaxed),
            },
        );
        service.process_ingress(
            DcMessage::Text(message.to_string())
                .for_generation(service.session_id.load(Ordering::Relaxed)),
        );
        assert_eq!(response.try_recv().unwrap().unwrap(), message);
        for _ in 0..64 {
            assert!(events.try_recv().is_ok());
        }
        assert!(events.try_recv().is_err());
    }

    #[test]
    fn chunk_waiter_merges_but_subscriber_receives_original_chunks_in_both_schemas() {
        for file_schema in [false, true] {
            let (service, _channel, _ingress, events) = make_service("open");
            service.subscribe("topic").unwrap();
            let mut first = json!({"type":"res", "topic":"topic"});
            let metadata = json!({"enable_chunking":true,"chunk_index":1,"total_chunk_num":2});
            let (index_path, payload_path) = if file_schema {
                first["info"] = json!({"file":metadata});
                first["info"]["file"]["data"] = json!([65]);
                ("/info/file/chunk_index", "/info/file/data")
            } else {
                first["data"] = json!({"content_info":metadata,"data":[65]});
                ("/data/content_info/chunk_index", "/data/data")
            };
            let (sender, mut response) = oneshot::channel();
            service.pending_callbacks.lock().unwrap().insert(
                DataChannelService::<MockDataChannel>::incoming_key(&first),
                PendingCallback {
                    token: Arc::new(()),
                    sender,
                    expectation: ResponseExpectation::any(),
                    session_id: service.session_id.load(Ordering::Relaxed),
                },
            );
            let mut second = first.clone();
            *second.pointer_mut(index_path).unwrap() = json!(2);
            *second.pointer_mut(payload_path).unwrap() = json!([66]);
            for (index, chunk) in [first, second].into_iter().enumerate() {
                service.process_ingress(
                    DcMessage::Text(chunk.to_string())
                        .for_generation(service.session_id.load(Ordering::Relaxed)),
                );
                let CallbackEvent::TopicCallback { payload, .. } = events.try_recv().unwrap()
                else {
                    panic!("expected original chunk callback");
                };
                assert_eq!(payload, chunk);
                if index == 0 {
                    assert!(matches!(
                        response.try_recv(),
                        Err(oneshot::error::TryRecvError::Empty)
                    ));
                }
            }
            let merged = response.try_recv().unwrap().unwrap();
            assert_eq!(merged.pointer(payload_path).unwrap(), &json!([65, 66]));
            assert!(events.try_recv().is_err());
        }
    }

    // ── subscribe ─────────────────────────────────────────────────────────────

    /// subscribe() sends a json subscribe message over the data channel.
    #[test]
    fn subscribe_sends_subscribe_message() {
        let (service, channel, _dc_tx, _cb_rx) = make_service("open");
        service
            .subscribe("rt/lf/sportmodestate")
            .expect("subscribe should succeed");

        let sent = channel.sent.lock().unwrap();
        assert_eq!(sent.len(), 1);
        let msg: Value = serde_json::from_str(&sent[0]).unwrap();
        assert_eq!(msg["type"], dc_types().subscribe);
        assert_eq!(msg["topic"], "rt/lf/sportmodestate");
    }

    /// Subscribing then injecting a matching text message causes TopicCallback.
    /// Data pattern mirrors sport_mode_state.py / integration.py.
    #[tokio::test]
    async fn subscribed_topic_dispatches_callback_event() {
        let (service, _channel, dc_tx, cb_rx) = make_service("open");

        service.subscribe("rt/lf/sportmodestate").unwrap();

        let state_msg = json!({
            "type": "msg",
            "topic": "rt/lf/sportmodestate",
            "data": {
                "mode": 1,
                "gait_type": 0,
                "foot_raise_height": 0.09,
                "position": [0.0, 0.0, 0.0],
                "body_height": 0.32,
                "velocity": [0.0, 0.0, 0.0],
                "yaw_speed": 0.0,
                "imu_state": {
                    "quaternion": [1.0, 0.0, 0.0, 0.0],
                    "gyroscope": [0.0, 0.0, 0.0],
                    "accelerometer": [0.0, 0.0, 9.81],
                    "rpy": [0.0, 0.0, 0.0],
                    "temperature": 36
                }
            }
        });

        dc_tx.send(DcMessage::Text(state_msg.to_string())).unwrap();
        tokio::time::sleep(Duration::from_millis(60)).await;

        let event = cb_rx
            .recv_timeout(Duration::from_millis(200))
            .expect("expected a TopicCallback event");
        match event {
            CallbackEvent::TopicCallback { topic, payload } => {
                assert_eq!(topic, "rt/lf/sportmodestate");
                assert_eq!(payload["data"]["mode"], 1);
            }
            other => panic!("Expected TopicCallback, got {:?}", other),
        }
    }

    // ── publish_request_new (sport mode pattern) ──────────────────────────────

    /// publish_request_new builds the correct api_id header payload.
    /// Mirrors sport_mode.py: publish_request_new(SPORT_MOD, {"api_id": GetState})
    #[tokio::test]
    async fn publish_request_new_builds_sport_payload() {
        let (service, channel, dc_tx, _cb_rx) = make_service("open");
        let request_type = dc_types().request;

        // Inject robot response so the call doesn't time out
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(20)).await;
            let resp = json!({
                "type": dc_types().response,
                "topic": "rt/api/sport/response",
                "data": {
                    "header": {
                        "identity": { "id": 4242, "api_id": 1034 },
                        "status": { "code": 0 }
                    },
                    "data": ""
                }
            });
            let _ = dc_tx.send(DcMessage::Text(resp.to_string()));
        });

        let _ = service
            .publish_request_new(
                "rt/api/sport/request",
                json!({"api_id": 1034, "id": 4242}), // SPORT_CMD["GetState"]
                Some(1.0),
            )
            .await
            .expect("matching sport response should resolve");

        let sent = channel.sent.lock().unwrap();
        let req: Value = serde_json::from_str(
            sent.iter()
                .find(|s| s.contains("\"type\":\"req\""))
                .expect("should contain a req message"),
        )
        .unwrap();

        assert_eq!(req["type"], request_type);
        assert_eq!(req["topic"], "rt/api/sport/request");
        assert_eq!(req["data"]["header"]["identity"]["api_id"], 1034);
        assert_eq!(req["data"]["header"]["identity"]["id"], 4242);
        // parameter must be present as a string field
        assert!(req["data"]["parameter"].is_string());
    }

    #[tokio::test]
    async fn publish_request_new_rejects_mismatched_response_topic() {
        let (service, _channel, dc_tx, _cb_rx) = make_service("open");

        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(20)).await;
            let resp = json!({
                "type": dc_types().response,
                "topic": "rt/api/vui/response",
                "data": {
                    "header": {
                        "identity": { "id": 5252, "api_id": 1005 },
                        "status": { "code": 0 }
                    },
                    "data": ""
                }
            });
            let _ = dc_tx.send(DcMessage::Text(resp.to_string()));
        });

        let result = service
            .publish_request_new(
                "rt/api/sport/request",
                json!({"api_id": 1005, "id": 5252}),
                Some(1.0),
            )
            .await;

        assert!(
            result.unwrap_err().contains("Response topic mismatch"),
            "wrong topic must not resolve as success"
        );
    }

    #[tokio::test]
    async fn publish_request_new_rejects_duplicate_inflight_id() {
        let (service, _channel, _dc_tx, _cb_rx) = make_service("open");
        let first_service = service.clone();

        let first = tokio::spawn(async move {
            first_service
                .publish_request_new(
                    "rt/api/sport/request",
                    json!({"api_id": 1034, "id": 6262}),
                    Some(0.2),
                )
                .await
        });

        tokio::time::sleep(Duration::from_millis(20)).await;
        let second = service
            .publish_request_new(
                "rt/api/vui/request",
                json!({"api_id": 1001, "id": 6262}),
                Some(0.2),
            )
            .await;

        assert!(
            second
                .unwrap_err()
                .contains("Duplicate in-flight request key"),
            "same wire id must not create a second waiter"
        );

        let _ = first.await.expect("first task should finish");
    }

    #[test]
    fn generated_identity_ids_are_unique_in_burst() {
        let (service, _channel, _dc_tx, _cb_rx) = make_service("open");
        let mut ids = HashSet::new();

        for _ in 0..2_000 {
            let id = service.generated_identity_id();
            assert!(ids.insert(id), "generated id collision: {id}");
        }
    }

    #[tokio::test]
    async fn start_new_session_fails_pending_requests() {
        let (service, _channel, _dc_tx, _cb_rx) = make_service("open");
        let request_service = service.clone();

        let pending = tokio::spawn(async move {
            request_service
                .publish_request_new(
                    "rt/api/sport/request",
                    json!({"api_id": 1034, "id": 7272}),
                    Some(5.0),
                )
                .await
        });

        tokio::time::sleep(Duration::from_millis(20)).await;
        service.start_new_session();

        let result = pending.await.expect("request task should finish");
        assert!(
            result.unwrap_err().contains("DataChannel session reset"),
            "session reset must fail stale pending requests"
        );
    }

    /// publish_request_new with a dict parameter (FrontFlip from sport_mode.py).
    #[tokio::test]
    async fn publish_request_new_with_dict_parameter() {
        let (service, channel, dc_tx, _cb_rx) = make_service("open");

        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(20)).await;
            let resp = json!({
                "type": dc_types().response,
                "topic": "rt/api/sport/response",
                "data": {
                    "header": {
                        "identity": { "id": 8383, "api_id": 1030 },
                        "status": { "code": 0 }
                    },
                    "data": ""
                }
            });
            let _ = dc_tx.send(DcMessage::Text(resp.to_string()));
        });

        let _ = service
            .publish_request_new(
                "rt/api/sport/request",
                json!({"api_id": 1030, "id": 8383, "parameter": {"data": true}}), // FrontFlip
                Some(1.0),
            )
            .await
            .expect("matching sport response should resolve");

        let sent = channel.sent.lock().unwrap();
        let req: Value = serde_json::from_str(
            sent.iter()
                .find(|s| s.contains("\"type\":\"req\""))
                .unwrap(),
        )
        .unwrap();
        // non-string parameter is JSON-serialized to a string
        assert!(
            req["data"]["parameter"].is_string(),
            "dict parameter should be stringified"
        );
    }

    // ── publish_without_callback ──────────────────────────────────────────────

    /// publish_without_callback sends a msg-type message with no pending waiter.
    /// Mirrors lidar_stream.py: publish_without_callback("rt/utlidar/switch", "on")
    #[test]
    fn publish_without_callback_sends_correct_message() {
        let (service, channel, _dc_tx, _cb_rx) = make_service("open");

        service
            .publish_without_callback("rt/utlidar/switch", Some(json!("on")), None)
            .expect("should succeed on open channel");

        let sent = channel.sent.lock().unwrap();
        assert_eq!(sent.len(), 1);
        let msg: Value = serde_json::from_str(&sent[0]).unwrap();
        assert_eq!(msg["type"], dc_types().msg);
        assert_eq!(msg["topic"], "rt/utlidar/switch");
        assert_eq!(msg["data"], "on");
    }

    /// publish_without_callback fails when channel is not open.
    #[test]
    fn publish_without_callback_fails_on_closed_channel() {
        let (service, _channel, _dc_tx, _cb_rx) = make_service("closed");
        assert!(service
            .publish_without_callback("rt/utlidar/switch", Some(json!("on")), None)
            .is_err());
    }

    // ── throttle / should_process ─────────────────────────────────────────────

    /// LiDAR topic is throttled: second immediate call returns false.
    #[test]
    fn throttle_limits_lidar_topic() {
        let (service, _channel, _dc_tx, _cb_rx) = make_service("open");
        let topic = "rt/utlidar/voxel_map_compressed";
        assert!(service.should_process(topic), "first call should pass");
        assert!(
            !service.should_process(topic),
            "immediate second call should be throttled"
        );
    }

    /// Non-throttled topics always pass.
    #[test]
    fn unthrottled_topics_always_pass() {
        let (service, _channel, _dc_tx, _cb_rx) = make_service("open");
        for _ in 0..5 {
            assert!(service.should_process("rt/lf/sportmodestate"));
        }
    }

    // ── decoder switch ────────────────────────────────────────────────────────

    /// default decoder is native and libvoxel is explicitly unsupported.
    #[test]
    fn set_decoder_validates_type() {
        let (service, _channel, _dc_tx, _cb_rx) = make_service("open");

        assert_eq!(service.decoder_name(), "NativeDecoder");
        assert!(service.set_decoder("native").is_ok());
        assert_eq!(service.decoder_name(), "NativeDecoder");

        assert!(service.set_decoder("libvoxel").is_err());
        assert_eq!(service.decoder_name(), "NativeDecoder");

        assert!(service.set_decoder("unknown_decoder").is_err());
    }

    // ── binary payload routing ────────────────────────────────────────────────

    /// Normal binary (header_1 != 2 bytes) with valid JSON header is parsed.
    #[test]
    fn binary_normal_header_parses_json() {
        let (service, _channel, _dc_tx, _cb_rx) = make_service("open");

        // Normal binary format: 2-byte json_len, 2-byte padding, then json, then binary data.
        // We make sure header_1 != 2 so it is not mistaken for a LiDAR packet.
        let json_bytes = br#"{"type":"msg","topic":"rt/custom"}"#;
        let json_len = json_bytes.len() as u16;
        assert_ne!(json_len, 2, "pick a json payload longer than 2 bytes");

        let mut buf = Vec::new();
        buf.extend_from_slice(&json_len.to_le_bytes()); // header_1 = len
        buf.extend_from_slice(&0u16.to_le_bytes()); // header_2
        buf.extend_from_slice(json_bytes);

        let result = service.deal_array_buffer(&buf);
        assert!(
            result.is_some(),
            "normal binary with valid JSON header should parse"
        );
        let parsed = result.unwrap();
        assert_eq!(parsed["type"], "msg");
        assert_eq!(parsed["topic"], "rt/custom");
    }

    /// Tagged LiDAR frames cannot fall back to unvalidated raw JSON.
    #[test]
    fn binary_lidar_tag_rejects_missing_metadata() {
        let (service, _channel, _dc_tx, _cb_rx) = make_service("open");

        // LiDAR binary format detected by: bytes 0-1 = u16(2), bytes 2-3 = u16(0).
        // deal_array_buffer_for_lidar(&buffer[4..]) is called on the remainder.
        // The remainder needs ≥ 8 bytes (4 for json_len, 4 reserved, then JSON).
        let json_bytes = br#"{"topic":"rt/utlidar/voxel_map_compressed"}"#;
        let json_len = json_bytes.len() as u32;
        let mut buf = Vec::new();
        buf.extend_from_slice(&2u16.to_le_bytes()); // header_1 = 2
        buf.extend_from_slice(&0u16.to_le_bytes()); // header_2 = 0  → lidar tag detected
        buf.extend_from_slice(&json_len.to_le_bytes()); // 4-byte json len (in lidar sub-buffer)
        buf.extend_from_slice(&[0u8; 4]); // 4-byte reserved (so json_start = 8 in sub-buffer)
        buf.extend_from_slice(json_bytes);

        assert!(service.deal_array_buffer(&buf).is_none());
    }

    // ── validation flow ───────────────────────────────────────────────────────

    /// "Validation Ok." message sets the data_channel_opened flag to true.
    #[tokio::test]
    async fn validation_ok_sets_channel_opened() {
        let (service, _channel, dc_tx, _cb_rx) = make_service("open");

        // Robot sends a key challenge first
        let challenge = json!({"type": "validation", "data": "test-key-xyz"});
        dc_tx.send(DcMessage::Text(challenge.to_string())).unwrap();
        tokio::time::sleep(Duration::from_millis(30)).await;

        // Then Validation Ok
        let ok = json!({"type": "validation", "data": "Validation Ok."});
        dc_tx.send(DcMessage::Text(ok.to_string())).unwrap();
        tokio::time::sleep(Duration::from_millis(60)).await;

        assert!(
            service.data_channel_opened.load(Ordering::Relaxed),
            "data_channel_opened should be true after Validation Ok."
        );
    }

    /// After validation Ok, heartbeat_running transitions to true.
    #[tokio::test]
    async fn validation_ok_starts_heartbeat() {
        let (service, _channel, dc_tx, _cb_rx) = make_service("open");

        let ok = json!({"type": "validation", "data": "Validation Ok."});
        dc_tx.send(DcMessage::Text(ok.to_string())).unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;

        assert!(
            service.heartbeat_running.load(Ordering::Relaxed),
            "heartbeat should start after validation"
        );
    }

    // ── chunked response ──────────────────────────────────────────────────────

    /// Chunked response with 2 chunks is reassembled before the callback fires.
    #[tokio::test]
    async fn chunked_response_reassembles_before_callback() {
        let (service, _channel, dc_tx, _cb_rx) = make_service("open");

        let response_type = dc_types().response;
        let key = format!("{} $ rt/api/audiohub/request", response_type);

        let (sx, mut rx) = oneshot::channel::<PortResult<Value>>();
        {
            service.pending_callbacks.lock().unwrap().insert(
                key,
                PendingCallback {
                    token: Arc::new(()),
                    sender: sx,
                    expectation: ResponseExpectation::any(),
                    session_id: service.session_id.load(Ordering::Relaxed),
                },
            );
        }

        // Chunk 1 of 2
        let chunk1 = json!({
            "type": response_type,
            "topic": "rt/api/audiohub/request",
            "data": {
                "content_info": { "enable_chunking": true, "chunk_index": 1, "total_chunk_num": 2 },
                "data": [72, 101, 108]   // "Hel"
            }
        });
        dc_tx.send(DcMessage::Text(chunk1.to_string())).unwrap();
        tokio::time::sleep(Duration::from_millis(40)).await;

        // Callback must NOT have fired yet
        assert!(
            matches!(
                rx.try_recv(),
                Err(tokio::sync::oneshot::error::TryRecvError::Empty)
            ),
            "callback must not fire after chunk 1 only"
        );

        // Chunk 2 of 2 (final)
        let chunk2 = json!({
            "type": response_type,
            "topic": "rt/api/audiohub/request",
            "data": {
                "content_info": { "enable_chunking": true, "chunk_index": 2, "total_chunk_num": 2 },
                "data": [108, 111]       // "lo"
            }
        });
        dc_tx.send(DcMessage::Text(chunk2.to_string())).unwrap();
        tokio::time::sleep(Duration::from_millis(60)).await;

        let result = rx
            .await
            .expect("callback should fire after final chunk")
            .expect("chunked response should validate");
        let merged: Vec<u8> = result["data"]["data"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_u64().unwrap() as u8)
            .collect();
        assert_eq!(merged, vec![72, 101, 108, 108, 111], "merged = 'Hello'");
    }

    // ── unsubscribe ───────────────────────────────────────────────────────────

    /// After unsubscribe, incoming topic messages are silently dropped.
    #[tokio::test]
    async fn unsubscribe_stops_dispatch() {
        let (service, _channel, dc_tx, cb_rx) = make_service("open");

        service.subscribe("rt/lf/lowstate").unwrap();
        service.unsubscribe("rt/lf/lowstate").unwrap();

        let msg = json!({"type":"msg","topic":"rt/lf/lowstate","data":{"motor_state":[]}});
        dc_tx.send(DcMessage::Text(msg.to_string())).unwrap();
        tokio::time::sleep(Duration::from_millis(60)).await;

        assert!(
            cb_rx.try_recv().is_err(),
            "no callback should fire after unsubscribe"
        );
    }

    // ── disable_traffic_saving ────────────────────────────────────────────

    /// disable_traffic_saving sends rtc_inner_req and returns true on success.
    #[tokio::test]
    async fn disable_traffic_saving_returns_true_on_success() {
        let (service, _channel, dc_tx, _cb_rx) = make_service("open");

        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(20)).await;
            let resp = json!({
                "type": dc_types().rtc_inner_req,
                "topic": "",
                "info": {"execution": "ok"}
            });
            let _ = dc_tx.send(DcMessage::Text(resp.to_string()));
        });

        let result = service.disable_traffic_saving(true).await;
        assert!(result.is_ok());
        assert!(result.unwrap());
    }

    /// disable_traffic_saving returns false when execution != "ok".
    #[tokio::test]
    async fn disable_traffic_saving_returns_false_on_failure() {
        let (service, _channel, dc_tx, _cb_rx) = make_service("open");

        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(20)).await;
            let resp = json!({
                "type": dc_types().rtc_inner_req,
                "topic": "",
                "info": {"execution": "failed"}
            });
            let _ = dc_tx.send(DcMessage::Text(resp.to_string()));
        });

        let result = service.disable_traffic_saving(true).await;
        assert!(result.is_ok());
        assert!(!result.unwrap());
    }

    // ── switch_video_channel / switch_audio_channel ───────────────────────

    /// switch_video_channel sends vid-type message with "on" or "off".
    #[test]
    fn switch_video_channel_sends_correct_message() {
        let (service, channel, _dc_tx, _cb_rx) = make_service("open");

        service.switch_video_channel(true).expect("should succeed");

        let sent = channel.sent.lock().unwrap();
        assert_eq!(sent.len(), 1);
        let msg: Value = serde_json::from_str(&sent[0]).unwrap();
        assert_eq!(msg["type"], dc_types().vid);
        assert_eq!(msg["data"], "on");
    }

    #[test]
    fn switch_video_channel_off_sends_off() {
        let (service, channel, _dc_tx, _cb_rx) = make_service("open");

        service.switch_video_channel(false).expect("should succeed");

        let sent = channel.sent.lock().unwrap();
        let msg: Value = serde_json::from_str(&sent[0]).unwrap();
        assert_eq!(msg["data"], "off");
    }

    /// switch_audio_channel sends aud-type message.
    #[test]
    fn switch_audio_channel_sends_correct_message() {
        let (service, channel, _dc_tx, _cb_rx) = make_service("open");

        service.switch_audio_channel(true).expect("should succeed");

        let sent = channel.sent.lock().unwrap();
        assert_eq!(sent.len(), 1);
        let msg: Value = serde_json::from_str(&sent[0]).unwrap();
        assert_eq!(msg["type"], dc_types().aud);
        assert_eq!(msg["data"], "on");
    }

    // ── wait_datachannel_open ──────────────────────────────────────────────

    /// wait_datachannel_open returns Ok immediately if already open.
    #[tokio::test]
    async fn wait_datachannel_open_succeeds_when_already_open() {
        let (service, _channel, _dc_tx, _cb_rx) = make_service("open");
        service.data_channel_opened.store(true, Ordering::Relaxed);

        let result = service.wait_datachannel_open(1.0).await;
        assert!(result.is_ok());
    }

    /// wait_datachannel_open times out if channel never opens.
    #[tokio::test]
    async fn wait_datachannel_open_times_out() {
        let (service, _channel, _dc_tx, _cb_rx) = make_service("open");

        let result = service.wait_datachannel_open(0.1).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("did not open in time"));
    }

    /// wait_datachannel_open succeeds when channel opens during wait.
    #[tokio::test]
    async fn wait_datachannel_open_succeeds_when_opened_later() {
        let (service, _channel, _dc_tx, _cb_rx) = make_service("open");
        let service_clone = service.clone();

        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            service_clone
                .data_channel_opened
                .store(true, Ordering::Relaxed);
        });

        let result = service.wait_datachannel_open(1.0).await;
        assert!(result.is_ok());
    }

    #[test]
    fn datachannel_status_reports_transport_and_heartbeat() {
        let (service, _channel, _dc_tx, _cb_rx) = make_service("open");
        service.data_channel_opened.store(true, Ordering::Relaxed);
        service.heartbeat_running.store(true, Ordering::Relaxed);
        service.heartbeat_last_response.store(
            DataChannelService::<MockDataChannel>::now_secs_u64().saturating_sub(3),
            Ordering::Relaxed,
        );

        assert_eq!(service.ready_state(), "open");
        assert!(service.is_open());
        assert!(service.heartbeat_running());
        assert!(service.last_heartbeat_age_s().unwrap() >= 3);
    }

    // ── stop_background_tasks ──────────────────────────────────────────────

    /// stop_background_tasks sets all background flags to false.
    #[test]
    fn stop_background_tasks_sets_flags_to_false() {
        let (service, _channel, _dc_tx, _cb_rx) = make_service("open");

        service.heartbeat_running.store(true, Ordering::Relaxed);
        service.network_probe_running.store(true, Ordering::Relaxed);
        service.data_channel_opened.store(true, Ordering::Relaxed);

        service.stop_background_tasks();

        assert!(!service.heartbeat_running.load(Ordering::Relaxed));
        assert!(!service.network_probe_running.load(Ordering::Relaxed));
        assert!(!service.data_channel_opened.load(Ordering::Relaxed));
    }

    // ── motion switcher flow (sport_mode.py pattern) ──────────────────────

    /// Motion switcher: sends two sequential requests with different api_ids.
    /// Mirrors sport_mode.py lines 61-84 pattern (check mode, then switch).
    #[test]
    fn motion_switcher_flow_sends_two_requests() {
        let (service, channel, _dc_tx, _cb_rx) = make_service("open");

        // First request: check current mode (api_id: 1001)
        let _ = service.publish_without_callback(
            "rt/api/motion_switcher/request",
            Some(json!({
                "header": {
                    "identity": {"api_id": 1001, "id": 0}
                },
                "parameter": ""
            })),
            Some(dc_types().request),
        );

        // Second request: switch to normal mode (api_id: 1002, parameter: {"name": "normal"})
        let _ = service.publish_without_callback(
            "rt/api/motion_switcher/request",
            Some(json!({
                "header": {
                    "identity": {"api_id": 1002, "id": 0}
                },
                "parameter": r#"{"name":"normal"}"#
            })),
            Some(dc_types().request),
        );

        let sent = channel.sent.lock().unwrap();
        let requests: Vec<Value> = sent
            .iter()
            .filter_map(|s| serde_json::from_str(s).ok())
            .filter(|v: &Value| v["type"] == dc_types().request)
            .collect();

        assert_eq!(requests.len(), 2, "should have sent 2 request messages");
        assert_eq!(requests[0]["topic"], "rt/api/motion_switcher/request");
        assert_eq!(requests[0]["data"]["header"]["identity"]["api_id"], 1001);
        assert_eq!(requests[1]["topic"], "rt/api/motion_switcher/request");
        assert_eq!(requests[1]["data"]["header"]["identity"]["api_id"], 1002);
    }
}

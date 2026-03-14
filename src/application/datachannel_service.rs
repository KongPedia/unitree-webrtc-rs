use crate::application::lidar_service::{LidarDecodeRequest, LidarMetadata, LidarWorkerPool};
use crate::domain::models::{CallbackEvent, DcMessage, RequestIdentity};
use crate::domain::ports::{DataChannelPort, PortResult};
use crate::infrastructure::security::encrypt_key;
use crate::interface::constants::data_channel_type;
use chrono::Local;
use crossbeam_channel::{Receiver, Sender, TrySendError};
use serde_json::{json, Map, Value};
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::oneshot;
use tracing::{error, info, warn};

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
    pending_callbacks: Arc<Mutex<HashMap<String, Vec<oneshot::Sender<Value>>>>>,
    subscriptions: Arc<Mutex<HashSet<String>>>,
    chunk_data_storage: Arc<Mutex<HashMap<String, Vec<Vec<u8>>>>>,
    validation_key: Arc<Mutex<String>>,
    callback_events_tx: Sender<CallbackEvent>,
    data_channel_opened: Arc<AtomicBool>,
    heartbeat_last_response: Arc<AtomicU64>,
    heartbeat_running: Arc<AtomicBool>,
    network_probe_running: Arc<AtomicBool>,
    throttle_limits: Arc<HashMap<String, f64>>,
    last_process_time: Arc<Mutex<HashMap<String, f64>>>,
    decoder_type: Arc<Mutex<String>>,
    lidar_worker_pool: Arc<LidarWorkerPool>,
    is_remote_connection: bool,
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
            validation_key: Arc::clone(&self.validation_key),
            callback_events_tx: self.callback_events_tx.clone(),
            data_channel_opened: Arc::clone(&self.data_channel_opened),
            heartbeat_last_response: Arc::clone(&self.heartbeat_last_response),
            heartbeat_running: Arc::clone(&self.heartbeat_running),
            network_probe_running: Arc::clone(&self.network_probe_running),
            throttle_limits: Arc::clone(&self.throttle_limits),
            last_process_time: Arc::clone(&self.last_process_time),
            decoder_type: Arc::clone(&self.decoder_type),
            lidar_worker_pool: Arc::clone(&self.lidar_worker_pool),
            is_remote_connection: self.is_remote_connection,
        }
    }
}

impl<T> DataChannelService<T>
where
    T: DataChannelPort + 'static,
{
    pub fn new(
        transport: Arc<T>,
        incoming_rx: Receiver<DcMessage>,
        callback_events_tx: Sender<CallbackEvent>,
        lidar_worker_pool: Arc<LidarWorkerPool>,
        is_remote_connection: bool,
    ) -> Self {
        let mut throttle_limits = HashMap::new();
        throttle_limits.insert("rt/utlidar/voxel_map_compressed".to_string(), 1.0 / 15.0);
        throttle_limits.insert(dc_types().vid.to_string(), 1.0 / 15.0);

        let service = Self {
            transport,
            pending_callbacks: Arc::new(Mutex::new(HashMap::new())),
            subscriptions: Arc::new(Mutex::new(HashSet::new())),
            chunk_data_storage: Arc::new(Mutex::new(HashMap::new())),
            validation_key: Arc::new(Mutex::new(String::new())),
            callback_events_tx,
            data_channel_opened: Arc::new(AtomicBool::new(false)),
            heartbeat_last_response: Arc::new(AtomicU64::new(0)),
            heartbeat_running: Arc::new(AtomicBool::new(false)),
            network_probe_running: Arc::new(AtomicBool::new(false)),
            throttle_limits: Arc::new(throttle_limits),
            last_process_time: Arc::new(Mutex::new(HashMap::new())),
            decoder_type: Arc::new(Mutex::new("libvoxel".to_string())),
            lidar_worker_pool,
            is_remote_connection,
        };

        let service_for_router = service.clone();
        thread::Builder::new()
            .name("unitree-dc-router".to_string())
            .spawn(move || {
                service_for_router.run_message_loop(incoming_rx);
            })
            .expect("failed to spawn datachannel router thread");

        service
    }

    pub async fn publish(
        &self,
        topic: &str,
        data: Option<Value>,
        msg_type: Option<&str>,
        timeout_secs: Option<f64>,
    ) -> PortResult<Value> {
        if self.transport.ready_state() != "open" {
            return Err("Data channel is not open".to_string());
        }

        let effective_type = msg_type.unwrap_or(dc_types().msg);
        let key = Self::generate_message_key(
            effective_type,
            topic,
            data.as_ref().and_then(Self::outgoing_identifier_from_data),
        );

        let (tx, rx) = oneshot::channel::<Value>();
        {
            let mut pending = self.pending_callbacks.lock().unwrap();
            pending.entry(key.clone()).or_default().push(tx);
        }

        if let Err(error) = self.send_message(topic, data, effective_type) {
            self.drop_pending_callbacks(&key);
            return Err(error);
        }

        let timeout = timeout_secs.unwrap_or(10.0);
        match tokio::time::timeout(Duration::from_secs_f64(timeout), rx).await {
            Ok(Ok(response)) => Ok(response),
            Ok(Err(_)) => {
                self.drop_pending_callbacks(&key);
                Err("Response receiver dropped".to_string())
            }
            Err(_) => {
                self.drop_pending_callbacks(&key);
                Err(format!(
                    "Publish timeout after {timeout:.1}s for topic {topic}"
                ))
            }
        }
    }

    pub fn publish_without_callback(
        &self,
        topic: &str,
        data: Option<Value>,
        msg_type: Option<&str>,
    ) -> PortResult<()> {
        if self.transport.ready_state() != "open" {
            return Err("Data channel is not open".to_string());
        }

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
            .unwrap_or_else(Self::generated_identity_id);

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

        self.publish(
            topic,
            Some(request_payload),
            Some(dc_types().request),
            timeout_secs,
        )
        .await
    }

    pub fn subscribe(&self, topic: &str) -> PortResult<()> {
        if self.transport.ready_state() != "open" {
            return Err("Data channel is not open".to_string());
        }

        {
            let mut subscriptions = self.subscriptions.lock().unwrap();
            subscriptions.insert(topic.to_string());
        }

        self.publish_without_callback(topic, None, Some(dc_types().subscribe))
    }

    pub fn unsubscribe(&self, topic: &str) -> PortResult<()> {
        if self.transport.ready_state() != "open" {
            return Err("Data channel is not open".to_string());
        }

        {
            let mut subscriptions = self.subscriptions.lock().unwrap();
            subscriptions.remove(topic);
        }

        self.publish_without_callback(topic, None, Some(dc_types().unsubscribe))
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
        let timeout = Duration::from_secs_f64(timeout_secs);
        let started_at = std::time::Instant::now();

        loop {
            if self.data_channel_opened.load(Ordering::Relaxed) {
                return Ok(());
            }

            if started_at.elapsed() > timeout {
                return Err("Data channel did not open in time".to_string());
            }

            tokio::time::sleep(Duration::from_millis(100)).await;
        }
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

    pub fn stop_background_tasks(&self) {
        self.heartbeat_running.store(false, Ordering::Relaxed);
        self.network_probe_running.store(false, Ordering::Relaxed);
        self.data_channel_opened.store(false, Ordering::Relaxed);
    }

    pub fn set_decoder(&self, decoder_type: &str) -> PortResult<()> {
        if decoder_type != "libvoxel" && decoder_type != "native" {
            return Err("Invalid decoder type. Choose 'libvoxel' or 'native'.".to_string());
        }

        let mut current = self.decoder_type.lock().unwrap();
        *current = decoder_type.to_string();
        Ok(())
    }

    pub fn decoder_name(&self) -> String {
        let current = self.decoder_type.lock().unwrap();
        match current.as_str() {
            "native" => "NativeDecoder".to_string(),
            _ => "LibVoxelDecoder".to_string(),
        }
    }

    fn run_message_loop(&self, incoming_rx: Receiver<DcMessage>) {
        while let Ok(message) = incoming_rx.recv() {
            match message {
                DcMessage::Text(text) => match serde_json::from_str::<Value>(&text) {
                    Ok(parsed) => {
                        self.run_resolve(&parsed);
                        self.handle_text_response(&parsed);
                        self.dispatch_subscription(&parsed);
                    }
                    Err(error) => {
                        warn!(
                            event = "datachannel_json_parse_failed",
                            error = %error,
                            "Failed to parse text datachannel message"
                        );
                    }
                },
                DcMessage::Binary(binary) => {
                    if let Some(parsed) = self.deal_array_buffer(&binary) {
                        self.run_resolve(&parsed);
                        self.dispatch_subscription(&parsed);
                    }
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

        let key = Self::incoming_key(message);
        let callbacks = {
            let mut pending = self.pending_callbacks.lock().unwrap();
            pending.remove(&key)
        };

        if let Some(callbacks) = callbacks {
            for callback in callbacks {
                let _ = callback.send(message.clone());
            }
        }
    }

    fn handle_chunked_response(&self, message: &Value) -> bool {
        let key = Self::incoming_key(message);

        let content_chunk = message
            .get("data")
            .and_then(|value| value.get("content_info"))
            .and_then(|value| value.get("enable_chunking"))
            .and_then(Value::as_bool)
            .unwrap_or(false);

        if content_chunk {
            let chunk_index = message
                .get("data")
                .and_then(|value| value.get("content_info"))
                .and_then(|value| value.get("chunk_index"))
                .and_then(Value::as_u64)
                .unwrap_or(0);
            let total_chunks = message
                .get("data")
                .and_then(|value| value.get("content_info"))
                .and_then(|value| value.get("total_chunk_num"))
                .and_then(Value::as_u64)
                .unwrap_or(0);

            let bytes = message
                .get("data")
                .and_then(|value| value.get("data"))
                .and_then(Self::value_to_chunk_bytes)
                .unwrap_or_default();

            if total_chunks == 0 || chunk_index == 0 {
                return false;
            }

            let mut storage = self.chunk_data_storage.lock().unwrap();
            storage.entry(key.clone()).or_default().push(bytes);

            if chunk_index < total_chunks {
                return true;
            }

            if let Some(chunks) = storage.remove(&key) {
                let merged = chunks.into_iter().flatten().collect::<Vec<u8>>();

                let mut merged_message = message.clone();
                merged_message["data"]["data"] = Self::bytes_to_json_array(&merged);

                let callbacks = {
                    let mut pending = self.pending_callbacks.lock().unwrap();
                    pending.remove(&key)
                };

                if let Some(callbacks) = callbacks {
                    for callback in callbacks {
                        let _ = callback.send(merged_message.clone());
                    }
                }
            }

            return true;
        }

        let file_chunk = message
            .get("info")
            .and_then(|value| value.get("file"))
            .and_then(|value| value.get("enable_chunking"))
            .and_then(Value::as_bool)
            .unwrap_or(false);

        if !file_chunk {
            return false;
        }

        let chunk_index = message
            .get("info")
            .and_then(|value| value.get("file"))
            .and_then(|value| value.get("chunk_index"))
            .and_then(Value::as_u64)
            .unwrap_or(0);
        let total_chunks = message
            .get("info")
            .and_then(|value| value.get("file"))
            .and_then(|value| value.get("total_chunk_num"))
            .and_then(Value::as_u64)
            .unwrap_or(0);

        let bytes = message
            .get("info")
            .and_then(|value| value.get("file"))
            .and_then(|value| value.get("data"))
            .and_then(Self::value_to_chunk_bytes)
            .unwrap_or_default();

        if total_chunks == 0 || chunk_index == 0 {
            return false;
        }

        let mut storage = self.chunk_data_storage.lock().unwrap();
        storage.entry(key.clone()).or_default().push(bytes);

        if chunk_index < total_chunks {
            return true;
        }

        if let Some(chunks) = storage.remove(&key) {
            let merged = chunks.into_iter().flatten().collect::<Vec<u8>>();

            let mut merged_message = message.clone();
            merged_message["info"]["file"]["data"] = Self::bytes_to_json_array(&merged);

            let callbacks = {
                let mut pending = self.pending_callbacks.lock().unwrap();
                pending.remove(&key)
            };

            if let Some(callbacks) = callbacks {
                for callback in callbacks {
                    let _ = callback.send(merged_message.clone());
                }
            }
        }

        true
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
            self.start_heartbeat();
            self.request_network_status_once();
            return;
        }

        if let Some(key) = message.get("data").and_then(Value::as_str) {
            {
                let mut validation_key = self.validation_key.lock().unwrap();
                *validation_key = key.to_string();
            }

            if let Ok(encrypted) = encrypt_key(key) {
                let _ =
                    self.send_message("", Some(Value::String(encrypted)), dc_types().validation);
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
                let _ =
                    self.send_message("", Some(Value::String(encrypted)), dc_types().validation);
            }
        }
    }

    fn handle_rtc_inner_req(&self, message: &Value) {
        if let Some(info) = message.get("info") {
            if info.get("req_type").and_then(Value::as_str) == Some("rtt_probe_send_from_mechine") {
                let _ = self.send_message("", Some(info.clone()), dc_types().rtc_inner_req);
            }

            if let Some(status) = info.get("status").and_then(Value::as_str) {
                match status {
                    "NetworkStatus.ON_4G_CONNECTED" => {
                        info!(
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
                        info!(
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
        error!(event = "go2_error", payload = %message, "Error message received from Go2");
    }

    fn dispatch_subscription(&self, message: &Value) {
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
            payload: message.clone(),
        };

        if let Err(error) = self.callback_events_tx.try_send(event) {
            match error {
                TrySendError::Full(_) => {
                    warn!(
                        event = "python_callback_drop",
                        topic = topic,
                        reason = "callback_queue_full",
                        "Dropped callback event due to backpressure"
                    );
                }
                TrySendError::Disconnected(_) => {
                    warn!(
                        event = "python_callback_drop",
                        topic = topic,
                        reason = "callback_dispatcher_disconnected",
                        "Dropped callback event because dispatcher is unavailable"
                    );
                }
            }
        }
    }

    fn send_message(&self, topic: &str, data: Option<Value>, msg_type: &str) -> PortResult<()> {
        let mut message = json!({
            "type": msg_type,
            "topic": topic,
        });

        if let Some(data) = data {
            message["data"] = data;
        }

        let text = serde_json::to_string(&message).map_err(|error| error.to_string())?;
        self.transport.send_text(&text)
    }

    fn deal_array_buffer(&self, buffer: &[u8]) -> Option<Value> {
        if buffer.len() < 4 {
            warn!(
                event = "binary_message_ignored",
                reason = "buffer_too_short",
                length = buffer.len(),
                "Binary datachannel payload is too short"
            );
            return None;
        }

        let header_1 = u16::from_le_bytes([buffer[0], buffer[1]]);
        let header_2 = u16::from_le_bytes([buffer[2], buffer[3]]);

        if header_1 == 2 && header_2 == 0 {
            self.deal_array_buffer_for_lidar(&buffer[4..])
        } else {
            self.deal_array_buffer_for_normal(buffer)
        }
    }

    fn deal_array_buffer_for_normal(&self, buffer: &[u8]) -> Option<Value> {
        if buffer.len() < 4 {
            return None;
        }

        let header_length = u16::from_le_bytes([buffer[0], buffer[1]]) as usize;
        let json_start = 4usize;
        let json_end = json_start.checked_add(header_length)?;
        if json_end > buffer.len() {
            warn!(
                event = "binary_message_ignored",
                reason = "invalid_normal_header_length",
                header_length,
                payload_length = buffer.len(),
                "Normal binary payload header length exceeds payload size"
            );
            return None;
        }

        let json_data = &buffer[json_start..json_end];
        let binary_data = &buffer[json_end..];
        let decoded_json = match serde_json::from_slice::<Value>(json_data) {
            Ok(value) => value,
            Err(error) => {
                warn!(
                    event = "binary_json_parse_failed",
                    reason = "normal",
                    error = %error,
                    "Failed to parse normal binary payload JSON header"
                );
                return None;
            }
        };

        self.process_binary_payload(decoded_json, binary_data)
    }

    fn deal_array_buffer_for_lidar(&self, buffer: &[u8]) -> Option<Value> {
        if buffer.len() < 8 {
            return None;
        }

        let header_length =
            u32::from_le_bytes([buffer[0], buffer[1], buffer[2], buffer[3]]) as usize;
        let json_start = 8usize;
        let json_end = json_start.checked_add(header_length)?;
        if json_end > buffer.len() {
            warn!(
                event = "binary_message_ignored",
                reason = "invalid_lidar_header_length",
                header_length,
                payload_length = buffer.len(),
                "LiDAR binary payload header length exceeds payload size"
            );
            return None;
        }

        let json_data = &buffer[json_start..json_end];
        let binary_data = &buffer[json_end..];
        let decoded_json = match serde_json::from_slice::<Value>(json_data) {
            Ok(value) => value,
            Err(error) => {
                warn!(
                    event = "binary_json_parse_failed",
                    reason = "lidar",
                    error = %error,
                    "Failed to parse LiDAR binary payload JSON header"
                );
                return None;
            }
        };

        self.process_binary_payload(decoded_json, binary_data)
    }

    fn process_binary_payload(&self, mut decoded_json: Value, binary_data: &[u8]) -> Option<Value> {
        let topic_or_type = decoded_json
            .get("topic")
            .and_then(Value::as_str)
            .or_else(|| decoded_json.get("type").and_then(Value::as_str))
            .unwrap_or_default()
            .to_string();

        if !self.should_process(&topic_or_type) {
            return None;
        }

        let metadata = decoded_json.get("data").and_then(LidarMetadata::from_json);
        let decoder_type = self.decoder_type.lock().unwrap().clone();

        if let Some(metadata) = metadata {
            if decoder_type == "native" {
                let request = LidarDecodeRequest {
                    topic: topic_or_type.clone(),
                    payload: decoded_json,
                    compressed_data: binary_data.to_vec(),
                    metadata,
                };

                if let Err(error) = self.lidar_worker_pool.submit(request) {
                    warn!(
                        event = "lidar_worker_submit_failed",
                        topic = topic_or_type,
                        error = %error,
                        "Failed to submit LiDAR decode request to worker pool"
                    );
                }
                return None;
            }
        }

        Self::inject_binary_data(&mut decoded_json, binary_data);
        Some(decoded_json)
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

    fn start_heartbeat(&self) {
        if self
            .heartbeat_running
            .compare_exchange(false, true, Ordering::Relaxed, Ordering::Relaxed)
            .is_err()
        {
            return;
        }

        let service = self.clone();
        thread::Builder::new()
            .name("unitree-heartbeat".to_string())
            .spawn(move || {
                while service.heartbeat_running.load(Ordering::Relaxed) {
                    let now = Self::now_secs_u64();
                    let last = service.heartbeat_last_response.load(Ordering::Relaxed);
                    if last > 0 && now.saturating_sub(last) > 6 {
                        warn!(
                            event = "heartbeat_timeout",
                            elapsed_sec = now.saturating_sub(last),
                            "Heartbeat timeout detected"
                        );
                        service.heartbeat_running.store(false, Ordering::Relaxed);
                        break;
                    }

                    if service.transport.ready_state() == "open" {
                        let formatted = Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
                        let data = json!({
                            "timeInStr": formatted,
                            "timeInNum": now,
                        });
                        let _ = service.send_message("", Some(data), dc_types().heartbeat);
                    }

                    thread::sleep(Duration::from_secs(2));
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

        let _ = self.send_message("", Some(data), dc_types().rtc_inner_req);
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
            return Some(text.as_bytes().to_vec());
        }

        let array = value.as_array()?;
        let mut output = Vec::with_capacity(array.len());
        for element in array {
            let byte = element.as_u64()?;
            output.push(byte as u8);
        }

        Some(output)
    }

    fn bytes_to_json_array(bytes: &[u8]) -> Value {
        Value::Array(
            bytes
                .iter()
                .map(|byte| Value::Number(serde_json::Number::from(*byte)))
                .collect::<Vec<_>>(),
        )
    }

    fn drop_pending_callbacks(&self, key: &str) {
        let mut pending = self.pending_callbacks.lock().unwrap();
        pending.remove(key);
    }

    fn generated_identity_id() -> i64 {
        let now_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_millis() as i64)
            .unwrap_or(0);
        let random_part: i64 = (rand::random::<u16>() % 1000) as i64;
        (now_ms % 2_147_483_648) + random_part
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::models::DcMessage;
    use crate::domain::ports::DataChannelPort;
    use crossbeam_channel::bounded;

    #[derive(Default)]
    struct MockDataChannel {
        state: &'static str,
        sent: Arc<Mutex<Vec<String>>>,
    }

    impl DataChannelPort for MockDataChannel {
        fn send_text(&self, message: &str) -> PortResult<()> {
            self.sent.lock().unwrap().push(message.to_string());
            Ok(())
        }

        fn send_binary(&self, _bytes: &[u8]) -> PortResult<()> {
            Ok(())
        }

        fn set_message_sender(
            &self,
            _sender: crossbeam_channel::Sender<DcMessage>,
        ) -> PortResult<()> {
            Ok(())
        }

        fn ready_state(&self) -> &'static str {
            self.state
        }
    }

    #[test]
    fn publish_request_builds_payload() {
        // Verify that publish_without_callback produces a correctly-shaped outgoing "msg"
        // frame over the DataChannel — this is the send path that publish_request shares.
        let (_dc_tx, dc_rx) = bounded::<DcMessage>(32);
        let (callback_tx, _callback_rx) = bounded::<CallbackEvent>(32);
        let lidar_pool = crate::application::lidar_service::create_worker_pool(callback_tx.clone());
        let channel = Arc::new(MockDataChannel {
            state: "open",
            sent: Arc::new(Mutex::new(Vec::new())),
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

    // ─────────────────────────────────────────────────────────────────────────
    // Helper: creates a DataChannelService with a MockDataChannel and returns
    // all channels needed for test assertions.
    // ─────────────────────────────────────────────────────────────────────────
    fn make_service(
        state: &'static str,
    ) -> (
        DataChannelService<MockDataChannel>,
        Arc<MockDataChannel>,
        crossbeam_channel::Sender<DcMessage>,
        crossbeam_channel::Receiver<CallbackEvent>,
    ) {
        let (dc_tx, dc_rx) = bounded::<DcMessage>(64);
        let (cb_tx, cb_rx) = bounded::<CallbackEvent>(64);
        let lidar_pool = crate::application::lidar_service::create_worker_pool(cb_tx.clone());
        let channel = Arc::new(MockDataChannel {
            state,
            sent: Arc::new(Mutex::new(Vec::new())),
        });
        let service =
            DataChannelService::new(Arc::clone(&channel), dc_rx, cb_tx, lidar_pool, false);
        (service, channel, dc_tx, cb_rx)
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
                "type": "res",
                "topic": "rt/api/sport/request",
                "data": {
                    "header": {
                        "identity": { "id": 0, "api_id": 1034 },
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
                json!({"api_id": 1034}), // SPORT_CMD["GetState"]
                Some(1.0),
            )
            .await;

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
        // parameter must be present as a string field
        assert!(req["data"]["parameter"].is_string());
    }

    /// publish_request_new with a dict parameter (FrontFlip from sport_mode.py).
    #[tokio::test]
    async fn publish_request_new_with_dict_parameter() {
        let (service, channel, dc_tx, _cb_rx) = make_service("open");

        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(20)).await;
            let resp = json!({"type": "res", "topic": "rt/api/sport/request", "data": {}});
            let _ = dc_tx.send(DcMessage::Text(resp.to_string()));
        });

        let _ = service
            .publish_request_new(
                "rt/api/sport/request",
                json!({"api_id": 1030, "parameter": {"data": true}}), // FrontFlip
                Some(1.0),
            )
            .await;

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

    /// set_decoder accepts "native" / "libvoxel", rejects unknown strings.
    #[test]
    fn set_decoder_validates_type() {
        let (service, _channel, _dc_tx, _cb_rx) = make_service("open");

        assert!(service.set_decoder("native").is_ok());
        assert_eq!(service.decoder_name(), "NativeDecoder");

        assert!(service.set_decoder("libvoxel").is_ok());
        assert_eq!(service.decoder_name(), "LibVoxelDecoder");

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

    /// Binary with header_1=2, header_2=0 is routed to the LiDAR path (deal_array_buffer_for_lidar).
    /// With a topic in JSON but no metadata, process_binary_payload returns Some (non-lidar path).
    /// With valid LiDAR metadata *and* the native decoder active, it would return None (submitted to worker).
    #[test]
    fn binary_lidar_tag_routes_to_lidar_path() {
        let (service, _channel, _dc_tx, _cb_rx) = make_service("open");

        // LiDAR binary format detected by: bytes 0-1 = u16(2), bytes 2-3 = u16(0).
        // deal_array_buffer_for_lidar(&buffer[4..]) is called on the remainder.
        // The remainder needs ≥ 8 bytes (4 for json_len, 4 reserved, then JSON).
        // With valid JSON but no "data.origin/resolution/src_size", metadata is None
        // → inject_binary_data → Some with binary data injected.
        let json_bytes = br#"{"topic":"rt/utlidar/voxel_map_compressed"}"#;
        let json_len = json_bytes.len() as u32;
        let mut buf = Vec::new();
        buf.extend_from_slice(&2u16.to_le_bytes()); // header_1 = 2
        buf.extend_from_slice(&0u16.to_le_bytes()); // header_2 = 0  → lidar tag detected
        buf.extend_from_slice(&json_len.to_le_bytes()); // 4-byte json len (in lidar sub-buffer)
        buf.extend_from_slice(&[0u8; 4]); // 4-byte reserved (so json_start = 8 in sub-buffer)
        buf.extend_from_slice(json_bytes);

        // No metadata in JSON → inject_binary_data path → Some
        let result = service.deal_array_buffer(&buf);
        assert!(
            result.is_some(),
            "lidar tag without metadata goes through inject_binary_data and returns Some"
        );
        let parsed = result.unwrap();
        assert_eq!(parsed["topic"], "rt/utlidar/voxel_map_compressed");
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

        let (sx, rx) = oneshot::channel::<Value>();
        {
            service
                .pending_callbacks
                .lock()
                .unwrap()
                .insert(key, vec![sx]);
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
        assert!(rx.is_empty(), "callback must not fire after chunk 1 only");

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

        let result = rx.await.expect("callback should fire after final chunk");
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
            cb_rx.is_empty(),
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

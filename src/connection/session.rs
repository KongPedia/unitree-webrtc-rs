//! Language-neutral owner of one validated WebRTC session and its bounded resources.
use super::lifecycle::{connect_validated_session, disconnect_session, SessionConnect};
use super::ConnectionService;
use crate::audio::transmit::service::AudioSource;
use crate::audio::AudioSender;
use crate::datachannel::lidar::LidarWorkerPool;
use crate::datachannel::DataChannelService;
use crate::infrastructure::rtc_engine::RtcEngine;
use crate::infrastructure::signaling_http::HttpSignalingClient;
use crate::protocol::constants::WebRTCConnectionMethod;
use crate::protocol::events::{event_channel, EventReceiver, EventSender};
use crate::protocol::models::IngressMessage;
use crate::protocol::ports::{DataChannelPort, PortResult, RtcEnginePort, SignalingPort};
use crossbeam_channel::{bounded, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::Mutex as AsyncMutex;

// Resource ceilings, not firmware maxima. Validate before queue/worker allocation.
const MAX_INCOMING_CAPACITY: usize = 4096;
const MAX_EVENT_CAPACITY: usize = 256;
const MAX_LIDAR_WORKERS: usize = 32;

/// Per-session ingress and control-callback capacities; media has a separate byte/count quota.
#[derive(Clone, Debug)]
pub struct SessionOptions {
    pub connection_method: WebRTCConnectionMethod,
    pub ip: Option<String>,
    pub validation_timeout: Duration,
    pub incoming_capacity: usize,
    pub event_capacity: usize,
    pub lidar_workers: usize,
}

impl SessionOptions {
    pub fn new(connection_method: WebRTCConnectionMethod, ip: Option<String>) -> Self {
        let (incoming_capacity, event_capacity) = match connection_method {
            WebRTCConnectionMethod::Remote => (512, 128),
            _ => (256, 64),
        };
        Self {
            connection_method,
            ip,
            validation_timeout: Duration::from_secs(5),
            incoming_capacity,
            event_capacity,
            lidar_workers: std::thread::available_parallelism().map_or(2, |n| n.get().clamp(2, 4)),
        }
    }

    fn validate(&self) -> PortResult<()> {
        for (name, value, maximum) in [
            (
                "incoming_capacity",
                self.incoming_capacity,
                MAX_INCOMING_CAPACITY,
            ),
            ("event_capacity", self.event_capacity, MAX_EVENT_CAPACITY),
            ("lidar_workers", self.lidar_workers, MAX_LIDAR_WORKERS),
        ] {
            if value == 0 || value > maximum {
                return Err(format!("{name} must be between 1 and {maximum}"));
            }
        }
        if self.validation_timeout.is_zero()
            || Instant::now()
                .checked_add(self.validation_timeout)
                .is_none()
        {
            return Err(
                "validation_timeout must be positive and within the monotonic clock range".into(),
            );
        }
        Ok(())
    }
}

/// One Rust-owned session, reusable by bindings without Python initialization.
///
/// Async operations require a Tokio runtime. Await `disconnect` for deterministic
/// transport cleanup. Final drop performs best-effort cleanup on the last active
/// runtime; a stopped runtime cannot guarantee asynchronous teardown.
/// The returned event receiver owns consumption; disconnect retains it for reconnect.
/// Outstanding subresource handles can retain queues/worker resources after owner drop.
pub struct UnitreeSession<S = HttpSignalingClient, E = RtcEngine>
where
    S: SignalingPort + 'static,
    E: RtcEnginePort + DataChannelPort + 'static,
{
    service: Arc<ConnectionService<S, E>>,
    engine: Arc<E>,
    datachannel: Arc<DataChannelService<E>>,
    audio_sender: Arc<AsyncMutex<Option<AudioSender>>>,
    validation_timeout: Duration,
    cleanup_runtime: Mutex<Option<tokio::runtime::Handle>>,
}

impl<S, E> UnitreeSession<S, E>
where
    S: SignalingPort + 'static,
    E: RtcEnginePort + DataChannelPort + 'static,
{
    /// Assemble the same queues, routing and lifecycle for real or offline adapters.
    /// Adapter factory receives the bounded ingress and callback-event producers.
    pub fn with_adapters<F>(
        options: SessionOptions,
        signaling: Arc<S>,
        create_engine: F,
    ) -> PortResult<(Self, EventReceiver)>
    where
        F: FnOnce(Sender<IngressMessage>, EventSender) -> Arc<E>,
    {
        options.validate()?;
        let (incoming_tx, incoming_rx) = bounded(options.incoming_capacity);
        let (events_tx, events_rx) = event_channel(options.event_capacity)?;
        let engine = create_engine(incoming_tx, events_tx.clone());
        let workers = Arc::new(LidarWorkerPool::new(
            events_tx.clone(),
            options.lidar_workers,
        ));
        let service = Arc::new(ConnectionService::new(
            signaling,
            engine.clone(),
            options.connection_method,
            options.ip,
        ));
        let datachannel = Arc::new(DataChannelService::new(
            engine.clone(),
            incoming_rx,
            events_tx,
            workers,
            options.connection_method == WebRTCConnectionMethod::Remote,
        ));
        Ok((
            Self {
                service,
                engine,
                datachannel,
                audio_sender: Arc::new(AsyncMutex::new(None)),
                validation_timeout: options.validation_timeout,
                cleanup_runtime: Mutex::new(None),
            },
            events_rx,
        ))
    }

    fn remember_runtime(&self) {
        *self.cleanup_runtime.lock().unwrap() = Some(tokio::runtime::Handle::current());
    }

    pub async fn connect(&self) -> PortResult<()> {
        self.remember_runtime();
        connect_validated_session(
            &self.service,
            &self.datachannel,
            &self.audio_sender,
            SessionConnect::Connect,
            self.validation_timeout.as_secs_f64(),
        )
        .await
    }

    pub async fn reconnect(&self) -> PortResult<()> {
        self.remember_runtime();
        connect_validated_session(
            &self.service,
            &self.datachannel,
            &self.audio_sender,
            SessionConnect::Reconnect,
            self.validation_timeout.as_secs_f64(),
        )
        .await
    }

    pub async fn disconnect(&self) {
        self.remember_runtime();
        disconnect_session(&self.service, &self.datachannel, &self.audio_sender).await;
    }

    /// True only after transport open and peer validation; false after either fails.
    pub fn is_connected(&self) -> bool {
        self.service.is_connected() && self.datachannel.is_open()
    }

    /// Retry validated sessions; explicit disconnect suppresses further attempts.
    pub async fn auto_reconnect(&self, max_retries: u32) -> bool {
        self.remember_runtime();
        for attempt in 0..max_retries {
            if self.service.intentional_disconnect() {
                return false;
            }
            match connect_validated_session(
                &self.service,
                &self.datachannel,
                &self.audio_sender,
                SessionConnect::Retry,
                self.validation_timeout.as_secs_f64(),
            )
            .await
            {
                Ok(()) => return true,
                Err(_) => tracing::warn!(
                    event = "webrtc_reconnect_attempt_failed",
                    stage = "connect_or_validation",
                    attempt = attempt + 1,
                    max_retries,
                    "Reconnect attempt failed; error details redacted"
                ),
            }
            if attempt + 1 < max_retries {
                tokio::time::sleep(Duration::from_millis(
                    200_u64.saturating_mul(1_u64 << attempt.min(8)),
                ))
                .await;
            }
        }
        false
    }

    /// Routing/RPC handle. It does not independently own a validated session.
    pub fn datachannel(&self) -> Arc<DataChannelService<E>> {
        Arc::clone(&self.datachannel)
    }
}

impl UnitreeSession {
    pub fn new(
        options: SessionOptions,
        aes_128_key: Option<String>,
    ) -> PortResult<(Self, EventReceiver)> {
        Self::with_adapters(
            options,
            Arc::new(HttpSignalingClient::new(aes_128_key)),
            |incoming, events| Arc::new(RtcEngine::new(incoming, events)),
        )
    }

    /// Replace playback under the same gate as connect/reconnect/disconnect.
    pub async fn play_audio(&self, source: AudioSource) -> PortResult<()> {
        self.remember_runtime();
        let _gate = self.service.lock_session().await;
        let result = async {
            *self.audio_sender.lock().await = None;
            if !self.is_connected() {
                return Err("Audio track not initialized. Call connect() first.".into());
            }
            let track =
                self.engine.get_audio_track().await.ok_or_else(|| {
                    "Audio track not initialized. Call connect() first.".to_string()
                })?;
            let sender = AudioSender::new(source, track)?;
            sender.play()?;
            *self.audio_sender.lock().await = Some(sender);
            Ok(())
        }
        .await;
        result
    }

    pub async fn stop_audio(&self) -> PortResult<()> {
        self.remember_runtime();
        let _gate = self.service.lock_session().await;
        let result = self
            .audio_sender
            .lock()
            .await
            .take()
            .map_or(Ok(()), |mut sender| sender.stop());
        result
    }
}

impl<S, E> Drop for UnitreeSession<S, E>
where
    S: SignalingPort + 'static,
    E: RtcEnginePort + DataChannelPort + 'static,
{
    fn drop(&mut self) {
        self.datachannel.stop_background_tasks();
        if let Ok(mut sender) = self.audio_sender.try_lock() {
            *sender = None;
        }
        if let Some(runtime) = self.cleanup_runtime.lock().unwrap().take() {
            let service = Arc::clone(&self.service);
            runtime.spawn(async move {
                service.disconnect().await;
            });
        }
    }
}

//! Language-neutral owner of one validated WebRTC session and its bounded resources.
use super::lifecycle::{
    connect_validated_session, disconnect_session, stop_audio_sender, AudioOperation,
    AudioPlayback, SessionConnect,
};
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
    audio_sender: Arc<AsyncMutex<AudioPlayback>>,
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
    ///
    /// # Errors
    /// Returns invalid resource options or a sanitized worker/router startup failure.
    /// Partially started idle LiDAR workers finish before an error is returned.
    pub fn with_adapters<F>(
        options: SessionOptions,
        signaling: Arc<S>,
        create_engine: F,
    ) -> PortResult<(Self, EventReceiver)>
    where
        F: FnOnce(Sender<IngressMessage>, EventSender) -> Arc<E>,
    {
        Self::with_adapters_and_spawner(options, signaling, create_engine, &mut |builder, job| {
            builder.spawn(job)
        })
    }

    fn with_adapters_and_spawner<F, W>(
        options: SessionOptions,
        signaling: Arc<S>,
        create_engine: F,
        spawn: &mut W,
    ) -> PortResult<(Self, EventReceiver)>
    where
        F: FnOnce(Sender<IngressMessage>, EventSender) -> Arc<E>,
        W: FnMut(
            std::thread::Builder,
            Box<dyn FnOnce() + Send>,
        ) -> std::io::Result<std::thread::JoinHandle<()>>,
    {
        options.validate()?;
        let (incoming_tx, incoming_rx) = bounded(options.incoming_capacity);
        let (events_tx, events_rx) = event_channel(options.event_capacity)?;
        let engine = create_engine(incoming_tx, events_tx.clone());
        let (workers, worker_handles) =
            LidarWorkerPool::try_new_with_spawner(events_tx.clone(), options.lidar_workers, spawn)?;
        let workers = Arc::new(workers);
        let service = Arc::new(ConnectionService::new(
            signaling,
            engine.clone(),
            options.connection_method,
            options.ip,
        ));
        let datachannel = DataChannelService::try_new_with_spawner(
            engine.clone(),
            incoming_rx,
            events_tx,
            workers,
            options.connection_method == WebRTCConnectionMethod::Remote,
            spawn,
        );
        let datachannel = match datachannel {
            Ok(datachannel) => Arc::new(datachannel),
            Err(error) => {
                // The failed router released its pool; no jobs exist before assembly completes.
                for worker in worker_handles {
                    let _ = worker.join();
                }
                return Err(error);
            }
        };
        Ok((
            Self {
                service,
                engine,
                datachannel,
                audio_sender: Arc::new(AsyncMutex::new(AudioPlayback::default())),
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
        let operation = AudioOperation::new(
            self.service.lock_session().await,
            Arc::clone(&self.audio_sender),
        );
        let result = async {
            stop_audio_sender(&self.audio_sender).await?;
            if !self.is_connected() {
                return Err("Audio track not initialized. Call connect() first.".into());
            }
            let track =
                self.engine.get_audio_track().await.ok_or_else(|| {
                    "Audio track not initialized. Call connect() first.".to_string()
                })?;
            let result = {
                let mut playback = self.audio_sender.lock().await;
                let sender = AudioSender::new(source, track)?;
                playback.play(sender)
            };
            if result.is_err() {
                let _ = stop_audio_sender(&self.audio_sender).await;
            }
            result
        }
        .await;
        operation.finish();
        result
    }

    pub async fn stop_audio(&self) -> PortResult<()> {
        self.remember_runtime();
        let operation = AudioOperation::new(
            self.service.lock_session().await,
            Arc::clone(&self.audio_sender),
        );
        let result = stop_audio_sender(&self.audio_sender).await;
        operation.finish();
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
        if let Some(runtime) = self.cleanup_runtime.lock().unwrap().take() {
            let service = Arc::clone(&self.service);
            let datachannel = Arc::clone(&self.datachannel);
            let audio = Arc::clone(&self.audio_sender);
            runtime.spawn(async move {
                disconnect_session(&service, &datachannel, &audio).await;
            });
        }
    }
}

#[cfg(test)]
mod audio_lifecycle_tests {
    use super::*;
    use crate::audio::transmit::service::test_support::delayed_audio;
    use std::sync::atomic::Ordering;

    fn offline_session() -> UnitreeSession {
        UnitreeSession::new(
            SessionOptions::new(WebRTCConnectionMethod::LocalAP, None),
            None,
        )
        .unwrap()
        .0
    }

    #[tokio::test(flavor = "current_thread")]
    async fn stop_audio_allows_timer_progress_and_observes_all_worker_completion() {
        let session = offline_session();
        let delayed = delayed_audio().await;
        session.audio_sender.lock().await.sender = Some(delayed.sender);
        let timer = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(5)).await;
            let _ = delayed.release.send(());
        });
        session.stop_audio().await.unwrap();
        timer.await.unwrap();
        assert!(
            delayed.native_joined.load(Ordering::SeqCst),
            "native join blocked the Tokio timer"
        );
        assert!(
            delayed.rtp_completed.load(Ordering::SeqCst),
            "stop returned before RTP task completion"
        );
    }
    #[derive(Clone, Copy, Debug)]
    enum CleanupBoundary {
        Stop,
        Replace,
        Disconnect,
        Drop,
    }

    #[tokio::test(flavor = "current_thread")]
    async fn replacement_disconnect_and_final_drop_keep_the_executor_live() {
        for boundary in [
            CleanupBoundary::Replace,
            CleanupBoundary::Disconnect,
            CleanupBoundary::Drop,
        ] {
            let session = offline_session();
            session.remember_runtime();
            let delayed = delayed_audio().await;
            session.audio_sender.lock().await.sender = Some(delayed.sender);
            let timer = tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(5)).await;
                let _ = delayed.release.send(());
            });
            match boundary {
                CleanupBoundary::Replace => {
                    assert!(session
                        .play_audio(AudioSource::File("offline-not-opened".into()))
                        .await
                        .is_err());
                }
                CleanupBoundary::Disconnect => session.disconnect().await,
                CleanupBoundary::Drop => drop(session),
                CleanupBoundary::Stop => unreachable!(),
            }
            timer.await.unwrap();
            tokio::time::timeout(Duration::from_secs(2), async {
                while !delayed.native_joined.load(Ordering::SeqCst)
                    || !delayed.rtp_completed.load(Ordering::SeqCst)
                {
                    tokio::task::yield_now().await;
                }
            })
            .await
            .unwrap_or_else(|_| panic!("{boundary:?} did not observe both workers"));
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn cancelled_audio_waiters_retain_the_gate_until_both_workers_finish() {
        for boundary in [
            CleanupBoundary::Stop,
            CleanupBoundary::Replace,
            CleanupBoundary::Disconnect,
        ] {
            let session = Arc::new(offline_session());
            let delayed = delayed_audio().await;
            session.audio_sender.lock().await.sender = Some(delayed.sender);
            let waiter = {
                let session = Arc::clone(&session);
                tokio::spawn(async move {
                    match boundary {
                        CleanupBoundary::Stop => session.stop_audio().await,
                        CleanupBoundary::Replace => {
                            session
                                .play_audio(AudioSource::File("offline-not-opened".into()))
                                .await
                        }
                        CleanupBoundary::Disconnect => {
                            session.disconnect().await;
                            Ok(())
                        }
                        CleanupBoundary::Drop => unreachable!(),
                    }
                })
            };
            tokio::time::timeout(Duration::from_secs(1), async {
                while delayed.bus_running.load(Ordering::SeqCst) {
                    tokio::task::yield_now().await;
                }
            })
            .await
            .unwrap();
            waiter.abort();
            assert!(waiter.await.unwrap_err().is_cancelled());
            let next_epoch_gate = session.service.lock_session();
            tokio::pin!(next_epoch_gate);
            assert!(
                tokio::time::timeout(Duration::from_millis(30), &mut next_epoch_gate)
                    .await
                    .is_err(),
                "{boundary:?} cancellation released the session gate before native cleanup"
            );
            delayed.release.send(()).unwrap();
            let gate = tokio::time::timeout(Duration::from_secs(1), next_epoch_gate)
                .await
                .unwrap();
            assert!(delayed.native_joined.load(Ordering::SeqCst));
            assert!(delayed.rtp_completed.load(Ordering::SeqCst));
            drop(gate);
        }
    }
}

#[cfg(test)]
mod construction_tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn construction_returns_error_and_releases_workers_and_adapters_at_each_spawn_failure() {
        for fail_at in 0..=2 {
            let completed = Arc::new(AtomicUsize::new(0));
            let mut attempts = 0;
            let mut weak_engine = None;
            let mut options = SessionOptions::new(WebRTCConnectionMethod::LocalAP, None);
            options.lidar_workers = 2;
            let result = UnitreeSession::with_adapters_and_spawner(
                options,
                Arc::new(HttpSignalingClient::new(None)),
                |incoming, events| {
                    let engine = Arc::new(RtcEngine::new(incoming, events));
                    weak_engine = Some(Arc::downgrade(&engine));
                    engine
                },
                &mut |builder, job| {
                    let attempt = attempts;
                    attempts += 1;
                    if attempt == fail_at {
                        return Err(std::io::Error::other("secret OS error detail"));
                    }
                    let completed = Arc::clone(&completed);
                    builder.spawn(move || {
                        job();
                        completed.fetch_add(1, Ordering::SeqCst);
                    })
                },
            );
            let error = result
                .err()
                .expect("construction must return a recoverable error");
            assert!(!error.contains("secret"));
            assert_eq!(attempts, fail_at + 1);
            assert_eq!(
                completed.load(Ordering::SeqCst),
                fail_at,
                "already started idle workers must complete before returning Err"
            );
            assert!(
                weak_engine.unwrap().upgrade().is_none(),
                "adapter leaked on failure"
            );
            let (session, events) = UnitreeSession::new(
                SessionOptions::new(WebRTCConnectionMethod::LocalAP, None),
                None,
            )
            .expect("a failed construction must not poison subsequent sessions");
            assert!(!session.is_connected());
            drop((session, events));
        }
    }
}

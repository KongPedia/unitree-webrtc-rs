use super::ConnectionService;
use crate::audio::AudioSender;
use crate::datachannel::DataChannelService;
#[cfg(test)]
use crate::protocol::events::event_channel;
use crate::protocol::ports::{DataChannelPort, PortResult, RtcEnginePort, SignalingPort};
use std::sync::Arc;
use tokio::sync::{Mutex as AsyncMutex, OwnedMutexGuard};

use super::service::ConnectionAttempt;

/// One sender and at most one owned cleanup task per serialized session.
#[derive(Default)]
pub(crate) struct AudioPlayback {
    pub(crate) sender: Option<AudioSender>,
    cleanup: Option<tokio::task::JoinHandle<PortResult<()>>>,
}

impl AudioPlayback {
    pub(crate) fn play(&mut self, sender: AudioSender) -> PortResult<()> {
        // Keep partial startup owned before the fallible native state transition.
        self.sender.insert(sender).play()
    }
}

pub(crate) async fn stop_audio_sender(audio: &Arc<AsyncMutex<AudioPlayback>>) -> PortResult<()> {
    let mut playback = audio.lock().await;
    if playback.cleanup.is_none() {
        if let Some(sender) = playback.sender.take() {
            playback.cleanup = Some(tokio::spawn(sender.shutdown()));
        }
    }
    let result = match playback.cleanup.as_mut() {
        // Poll the stored handle in place: cancellation must retain completion ownership.
        Some(cleanup) => cleanup
            .await
            .unwrap_or_else(|_| Err("Audio cleanup task failed".into())),
        None => Ok(()),
    };
    playback.cleanup.take();
    result
}

pub(crate) struct AudioOperation {
    gate: Option<OwnedMutexGuard<()>>,
    audio: Arc<AsyncMutex<AudioPlayback>>,
    runtime: tokio::runtime::Handle,
}

impl AudioOperation {
    pub(crate) fn new(gate: OwnedMutexGuard<()>, audio: Arc<AsyncMutex<AudioPlayback>>) -> Self {
        Self {
            gate: Some(gate),
            audio,
            runtime: tokio::runtime::Handle::current(),
        }
    }

    pub(crate) fn finish(mut self) {
        self.gate.take();
    }
}

impl Drop for AudioOperation {
    fn drop(&mut self) {
        if let Some(gate) = self.gate.take() {
            let audio = Arc::clone(&self.audio);
            self.runtime.spawn(async move {
                let _gate = gate;
                if stop_audio_sender(&audio).await.is_err() {
                    tracing::warn!(event = "audio_cleanup_failed", "Audio cleanup failed");
                }
            });
        }
    }
}

pub(crate) enum SessionConnect {
    Connect,
    Reconnect,
    Retry,
}

struct SessionAttempt<E: RtcEnginePort + DataChannelPort + 'static> {
    connection: Option<ConnectionAttempt<E>>,
    datachannel: DataChannelService<E>,
    audio: Arc<AsyncMutex<AudioPlayback>>,
    runtime: tokio::runtime::Handle,
}

impl<E: RtcEnginePort + DataChannelPort + 'static> SessionAttempt<E> {
    fn finish(mut self) {
        if let Some(connection) = self.connection.take() {
            connection.finish();
        }
    }
}

impl<E: RtcEnginePort + DataChannelPort + 'static> Drop for SessionAttempt<E> {
    fn drop(&mut self) {
        if let Some(connection) = self.connection.take() {
            self.datachannel.stop_background_tasks();
            let audio = Arc::clone(&self.audio);
            self.runtime.spawn(async move {
                if stop_audio_sender(&audio).await.is_err() {
                    tracing::warn!(event = "audio_cleanup_failed", "Audio cleanup failed");
                }
                // ConnectionAttempt retains the gate through its RTC rollback too.
                drop(connection);
            });
        }
    }
}

pub(crate) async fn connect_validated_session<S, E>(
    service: &ConnectionService<S, E>,
    datachannel: &DataChannelService<E>,
    audio_sender: &Arc<AsyncMutex<AudioPlayback>>,
    mode: SessionConnect,
    validation_timeout_secs: f64,
) -> PortResult<()>
where
    S: SignalingPort,
    E: RtcEnginePort + DataChannelPort + 'static,
{
    let mut attempt = SessionAttempt {
        connection: Some(service.begin_attempt().await),
        datachannel: datachannel.clone(),
        audio: Arc::clone(audio_sender),
        runtime: tokio::runtime::Handle::current(),
    };
    if matches!(mode, SessionConnect::Retry) && service.intentional_disconnect() {
        attempt.finish();
        return Err("Reconnect cancelled by intentional disconnect".into());
    }
    let result = attempt
        .connection
        .as_mut()
        .expect("active attempt")
        .run(async {
            stop_audio_sender(audio_sender).await?;
            datachannel.stop_background_tasks();
            datachannel.prepare_ingress()?;
            service
                .connect_in_attempt(!matches!(mode, SessionConnect::Connect))
                .await?;
            datachannel
                .wait_datachannel_open(validation_timeout_secs)
                .await
        })
        .await;
    if result.is_err() {
        let _ = stop_audio_sender(audio_sender).await;
        datachannel.stop_background_tasks();
        service.rollback_connection().await;
    }
    attempt.finish();
    result
}

pub(crate) async fn disconnect_session<S, E>(
    service: &ConnectionService<S, E>,
    datachannel: &DataChannelService<E>,
    audio_sender: &Arc<AsyncMutex<AudioPlayback>>,
) where
    S: SignalingPort,
    E: RtcEnginePort + DataChannelPort + 'static,
{
    service.request_disconnect();
    let attempt = SessionAttempt {
        connection: Some(service.begin_attempt().await),
        datachannel: datachannel.clone(),
        audio: Arc::clone(audio_sender),
        runtime: tokio::runtime::Handle::current(),
    };
    if stop_audio_sender(audio_sender).await.is_err() {
        tracing::warn!(event = "audio_cleanup_failed", "Audio cleanup failed");
    }
    datachannel.stop_background_tasks();
    service.rollback_connection().await;
    attempt.finish();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::datachannel::lidar;
    use crate::protocol::constants::WebRTCConnectionMethod;
    use crate::protocol::models::{DcMessage, IngressMessage};
    use crossbeam_channel::bounded;
    use std::sync::Arc;

    struct SessionTransport {
        incoming: crossbeam_channel::Sender<IngressMessage>,
        ready: std::sync::atomic::AtomicBool,
        validate: std::sync::atomic::AtomicBool,
        closes: std::sync::atomic::AtomicUsize,
        generation: std::sync::atomic::AtomicU64,
    }

    impl SignalingPort for SessionTransport {
        fn exchange_sdp<'a>(
            &'a self,
            _: &'a str,
            _: &'a str,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = PortResult<String>> + Send + 'a>>
        {
            Box::pin(async { Ok("answer".into()) })
        }
    }

    impl RtcEnginePort for SessionTransport {
        fn prepare_offer<'a>(
            &'a self,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = PortResult<String>> + Send + 'a>>
        {
            Box::pin(async { Ok("offer".into()) })
        }
        fn apply_answer<'a>(
            &'a self,
            _: &'a str,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = PortResult<()>> + Send + 'a>>
        {
            Box::pin(async move {
                self.ready.store(true, std::sync::atomic::Ordering::SeqCst);
                if self.validate.load(std::sync::atomic::Ordering::SeqCst) {
                    self.incoming
                        .send(
                            DcMessage::Text(
                                r#"{"type":"validation","data":"Validation Ok."}"#.into(),
                            )
                            .for_generation(
                                self.generation.load(std::sync::atomic::Ordering::SeqCst),
                            ),
                        )
                        .unwrap();
                }
                Ok(())
            })
        }
        fn close<'a>(
            &'a self,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'a>> {
            Box::pin(async move {
                self.ready.store(false, std::sync::atomic::Ordering::SeqCst);
                self.closes
                    .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            })
        }
    }

    impl DataChannelPort for SessionTransport {
        fn send_text(&self, _: &str) -> PortResult<()> {
            Ok(())
        }
        fn send_binary(&self, _: &[u8]) -> PortResult<()> {
            Ok(())
        }
        fn set_message_sender(
            &self,
            _: crossbeam_channel::Sender<IngressMessage>,
        ) -> PortResult<()> {
            Ok(())
        }
        fn set_ingress_generation(&self, generation: u64) -> PortResult<()> {
            self.generation
                .store(generation, std::sync::atomic::Ordering::SeqCst);
            Ok(())
        }
        fn ready_state(&self) -> &'static str {
            if self.ready.load(std::sync::atomic::Ordering::SeqCst) {
                "open"
            } else {
                "closed"
            }
        }
    }

    #[tokio::test]
    async fn shared_connect_path_requires_fresh_validation_and_rolls_back_failure() {
        use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
        let (incoming, incoming_rx) = bounded(4);
        let (callback_tx, _callback_rx) = event_channel(4).unwrap();
        let transport = Arc::new(SessionTransport {
            incoming,
            ready: AtomicBool::new(false),
            validate: AtomicBool::new(true),
            closes: AtomicUsize::new(0),
            generation: std::sync::atomic::AtomicU64::new(0),
        });
        let service = ConnectionService::new(
            Arc::clone(&transport),
            Arc::clone(&transport),
            WebRTCConnectionMethod::LocalSTA,
            Some("10.0.0.1".into()),
        );
        let workers = Arc::new(lidar::LidarWorkerPool::new(callback_tx.clone(), 1));
        let datachannel = DataChannelService::new(
            Arc::clone(&transport),
            incoming_rx,
            callback_tx,
            workers,
            false,
        );
        connect_validated_session(
            &service,
            &datachannel,
            &Arc::new(AsyncMutex::new(AudioPlayback::default())),
            SessionConnect::Connect,
            1.0,
        )
        .await
        .unwrap();
        assert!(service.is_connected() && datachannel.is_open());
        transport.validate.store(false, Ordering::SeqCst);
        let result = connect_validated_session(
            &service,
            &datachannel,
            &Arc::new(AsyncMutex::new(AudioPlayback::default())),
            SessionConnect::Reconnect,
            0.01,
        )
        .await;
        assert!(result.is_err(), "old validation must not satisfy reconnect");
        assert!(!service.is_connected());
        assert!(!datachannel.is_open());
        assert!(!datachannel.heartbeat_running());
        assert!(
            !service.intentional_disconnect(),
            "rollback must not suppress automatic retries"
        );
        assert_eq!(transport.closes.load(Ordering::SeqCst), 2);
    }
    #[tokio::test]
    async fn explicit_disconnect_interrupts_validation() {
        use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
        let (incoming, incoming_rx) = bounded(4);
        let (callback_tx, _callback_rx) = event_channel(4).unwrap();
        let transport = Arc::new(SessionTransport {
            incoming,
            ready: AtomicBool::new(false),
            validate: AtomicBool::new(false),
            closes: AtomicUsize::new(0),
            generation: std::sync::atomic::AtomicU64::new(0),
        });
        let service = Arc::new(ConnectionService::new(
            Arc::clone(&transport),
            Arc::clone(&transport),
            WebRTCConnectionMethod::LocalSTA,
            Some("10.0.0.1".into()),
        ));
        let workers = Arc::new(lidar::LidarWorkerPool::new(callback_tx.clone(), 1));
        let datachannel = Arc::new(DataChannelService::new(
            Arc::clone(&transport),
            incoming_rx,
            callback_tx,
            workers,
            false,
        ));
        let task_service = Arc::clone(&service);
        let task_dc = Arc::clone(&datachannel);
        let task = tokio::spawn(async move {
            connect_validated_session(
                &task_service,
                &task_dc,
                &Arc::new(AsyncMutex::new(AudioPlayback::default())),
                SessionConnect::Connect,
                5.0,
            )
            .await
        });
        tokio::time::timeout(std::time::Duration::from_secs(1), async {
            while !transport.ready.load(Ordering::SeqCst) {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        tokio::time::timeout(std::time::Duration::from_millis(200), service.disconnect())
            .await
            .expect("disconnect must interrupt validation");
        assert!(task.await.unwrap().unwrap_err().contains("cancel"));
        tokio::time::timeout(std::time::Duration::from_secs(1), async {
            while transport.closes.load(Ordering::SeqCst) == 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert_eq!(transport.closes.load(Ordering::SeqCst), 2);
        assert!(!service.is_connected());
        assert!(!datachannel.is_open());
        assert!(!datachannel.heartbeat_running());
    }
    #[tokio::test]
    async fn cancelling_validation_resets_core_and_closes_session() {
        use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
        let (incoming, incoming_rx) = bounded(4);
        let (callback_tx, _callback_rx) = event_channel(4).unwrap();
        let transport = Arc::new(SessionTransport {
            incoming,
            ready: AtomicBool::new(false),
            validate: AtomicBool::new(false),
            closes: AtomicUsize::new(0),
            generation: std::sync::atomic::AtomicU64::new(0),
        });
        let service = Arc::new(ConnectionService::new(
            Arc::clone(&transport),
            Arc::clone(&transport),
            WebRTCConnectionMethod::LocalSTA,
            Some("10.0.0.1".into()),
        ));
        let workers = Arc::new(lidar::LidarWorkerPool::new(callback_tx.clone(), 1));
        let datachannel = Arc::new(DataChannelService::new(
            Arc::clone(&transport),
            incoming_rx,
            callback_tx,
            workers,
            false,
        ));
        let task_service = Arc::clone(&service);
        let task_dc = Arc::clone(&datachannel);
        let task = tokio::spawn(async move {
            connect_validated_session(
                &task_service,
                &task_dc,
                &Arc::new(AsyncMutex::new(AudioPlayback::default())),
                SessionConnect::Connect,
                5.0,
            )
            .await
        });
        tokio::time::timeout(std::time::Duration::from_secs(1), async {
            while !transport.ready.load(Ordering::SeqCst) {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        task.abort();
        task.await.unwrap_err();
        tokio::time::timeout(std::time::Duration::from_secs(1), async {
            while transport.closes.load(Ordering::SeqCst) == 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert_eq!(transport.closes.load(Ordering::SeqCst), 1);
        assert!(!service.is_connected());
        assert!(!datachannel.is_open());
        assert!(!datachannel.heartbeat_running());
    }
    #[tokio::test(flavor = "current_thread")]
    async fn failed_playback_start_remains_owned_until_cleanup_completes() {
        use crate::audio::transmit::service::test_support::delayed_audio;
        use std::sync::atomic::Ordering;
        let audio = Arc::new(AsyncMutex::new(AudioPlayback::default()));
        let delayed = delayed_audio().await;
        delayed.bus_running.store(false, Ordering::SeqCst);
        assert!(audio.lock().await.play(delayed.sender).is_err());
        let timer = tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            let _ = delayed.release.send(());
        });
        stop_audio_sender(&audio).await.unwrap();
        timer.await.unwrap();
        assert!(delayed.native_joined.load(Ordering::SeqCst));
        assert!(delayed.rtp_completed.load(Ordering::SeqCst));
        assert!(audio.lock().await.sender.is_none());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn cancelled_connect_and_reconnect_wait_for_audio_before_the_next_epoch() {
        use crate::audio::transmit::service::test_support::delayed_audio;
        use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
        use std::time::Duration;
        for mode in [SessionConnect::Connect, SessionConnect::Reconnect] {
            let (incoming, incoming_rx) = bounded(4);
            let (callback_tx, _callback_rx) = event_channel(4).unwrap();
            let transport = Arc::new(SessionTransport {
                incoming,
                ready: AtomicBool::new(false),
                validate: AtomicBool::new(true),
                closes: AtomicUsize::new(0),
                generation: std::sync::atomic::AtomicU64::new(0),
            });
            let service = Arc::new(ConnectionService::new(
                Arc::clone(&transport),
                Arc::clone(&transport),
                WebRTCConnectionMethod::LocalSTA,
                Some("10.0.0.1".into()),
            ));
            let workers = Arc::new(lidar::LidarWorkerPool::new(callback_tx.clone(), 1));
            let datachannel = Arc::new(DataChannelService::new(
                Arc::clone(&transport),
                incoming_rx,
                callback_tx,
                workers,
                false,
            ));
            let audio = Arc::new(AsyncMutex::new(AudioPlayback::default()));
            let delayed = delayed_audio().await;
            audio.lock().await.sender = Some(delayed.sender);
            let initial_epoch = transport.generation.load(Ordering::SeqCst);
            let connect = {
                let service = Arc::clone(&service);
                let datachannel = Arc::clone(&datachannel);
                let audio = Arc::clone(&audio);
                tokio::spawn(async move {
                    connect_validated_session(&service, &datachannel, &audio, mode, 1.0).await
                })
            };
            tokio::time::timeout(Duration::from_secs(1), async {
                while delayed.bus_running.load(Ordering::SeqCst) {
                    tokio::task::yield_now().await;
                }
            })
            .await
            .unwrap();
            assert_eq!(
                transport.generation.load(Ordering::SeqCst),
                initial_epoch,
                "the next ingress epoch started before old audio cleanup"
            );
            connect.abort();
            assert!(connect.await.unwrap_err().is_cancelled());
            let next_epoch = service.lock_session();
            tokio::pin!(next_epoch);
            assert!(
                tokio::time::timeout(Duration::from_millis(30), &mut next_epoch)
                    .await
                    .is_err()
            );
            delayed.release.send(()).unwrap();
            drop(
                tokio::time::timeout(Duration::from_secs(1), next_epoch)
                    .await
                    .unwrap(),
            );
            assert!(delayed.native_joined.load(Ordering::SeqCst));
            assert!(delayed.rtp_completed.load(Ordering::SeqCst));
            assert!(!service.is_connected());
            connect_validated_session(&service, &datachannel, &audio, SessionConnect::Connect, 1.0)
                .await
                .unwrap();
            assert!(service.is_connected() && datachannel.is_open());
            disconnect_session(&service, &datachannel, &audio).await;
        }
    }
}

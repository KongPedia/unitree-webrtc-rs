use crate::diagnostics::RepeatLog;
use crate::infrastructure::gstreamer_util::ensure_gst_init;
use bytes::Bytes;
use gstreamer as gst;
use gstreamer::prelude::*;
use gstreamer_app as gst_app;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::time::Instant;
use tracing::{debug, error, info, warn};
use webrtc::rtp::packet::Packet as RtpPacket;
use webrtc::track::track_local::track_local_static_rtp::TrackLocalStaticRTP;
use webrtc::track::track_local::TrackLocalWriter;
use webrtc::util::Unmarshal;

#[derive(Clone)]
pub enum AudioSource {
    File(String),
    Url(String),
}

impl AudioSource {
    fn kind(&self) -> &'static str {
        match self {
            Self::File(_) => "file",
            Self::Url(_) => "url",
        }
    }

    fn to_uri(&self) -> Result<String, String> {
        let uri = match self {
            Self::File(path) => {
                let path = std::path::Path::new(path)
                    .canonicalize()
                    .map_err(|_| "Failed to resolve audio file path".to_string())?;
                if !path.is_file() {
                    return Err("Audio source must be a file".into());
                }
                gst::glib::filename_to_uri(path, None)
                    .map_err(|_| "Failed to construct audio file URI".to_string())?
                    .to_string()
            }
            Self::Url(url) => {
                if url.chars().any(char::is_control) {
                    return Err("Audio URL contains control characters".into());
                }
                url.clone()
            }
        };
        // Source handler selection validates installed schemes without starting playback.
        gst::Element::make_from_uri(gst::URIType::Src, &uri, None)
            .map_err(|_| "Unsupported or invalid audio source URI".to_string())?;
        Ok(uri)
    }
}

pub struct AudioSender {
    pipeline: gst::Pipeline,
    is_playing: Arc<AtomicBool>,
    bus_running: Arc<AtomicBool>,
    packet_count: Arc<AtomicU64>,
    source_type: AudioSource,
    task_handle: Option<tokio::task::JoinHandle<()>>,
    bus_handle: Option<thread::JoinHandle<()>>,
    rtp_tx: Option<mpsc::Sender<Bytes>>,
    native_shutdown: bool,
}

impl AudioSender {
    /// Create a new audio sender with GStreamer pipeline
    /// Pipeline: uridecodebin → audioconvert → audioresample → opusenc → rtpopuspay → appsink
    pub fn new(source: AudioSource, track: Arc<TrackLocalStaticRTP>) -> Result<Self, String> {
        ensure_gst_init()?;

        let uri = source.to_uri()?;
        info!(
            event = "audio_sender_init",
            backend = "gstreamer",
            source_type = source.kind(),
            "Creating audio sender pipeline"
        );

        let pipeline = gst::parse::launch(
            "uridecodebin name=src \
             ! audioconvert \
             ! audioresample \
             ! audio/x-raw,rate=48000,channels=2,format=S16LE \
             ! opusenc \
             ! rtpopuspay \
             ! appsink name=sink emit-signals=false sync=true max-buffers=2 drop=true",
        )
        .map_err(|e| format!("Failed to create GStreamer audio pipeline: {e}"))?
        .downcast::<gst::Pipeline>()
        .map_err(|_| "Pipeline element is not a Pipeline".to_string())?;
        pipeline
            .by_name("src")
            .ok_or_else(|| "Audio source element not found".to_string())?
            .set_property("uri", &uri);

        let appsink = pipeline
            .by_name("sink")
            .ok_or_else(|| "appsink element not found in pipeline".to_string())?
            .downcast::<gst_app::AppSink>()
            .map_err(|_| "Element 'sink' is not an AppSink".to_string())?;

        let is_playing = Arc::new(AtomicBool::new(false));
        let bus_running = Arc::new(AtomicBool::new(true));
        let packet_count = Arc::new(AtomicU64::new(0));

        // Create channel for RTP packets (GStreamer thread → async task)
        let (rtp_tx, mut rtp_rx) = mpsc::channel::<Bytes>(8);
        let rtp_tx_clone = rtp_tx.clone();

        // Set up appsink callback to forward RTP packets to channel
        let is_playing_clone = Arc::clone(&is_playing);
        let queue_drops = RepeatLog::new("audio_rtp_queue_drop", "full");
        appsink.set_callbacks(
            gst_app::AppSinkCallbacks::builder()
                .new_sample(move |sink| {
                    if !is_playing_clone.load(Ordering::Relaxed) {
                        return Ok(gst::FlowSuccess::Ok);
                    }

                    let sample = sink.pull_sample().map_err(|_| gst::FlowError::Error)?;
                    let buffer = sample.buffer().ok_or(gst::FlowError::Error)?;

                    let map = buffer.map_readable().map_err(|_| gst::FlowError::Error)?;

                    // Copy RTP packet data to channel
                    let rtp_data = Bytes::copy_from_slice(map.as_slice());

                    // Non-blocking send to async task
                    match rtp_tx_clone.try_send(rtp_data) {
                        Ok(()) => {}
                        Err(mpsc::error::TrySendError::Full(_)) => queue_drops.record(),
                        Err(mpsc::error::TrySendError::Closed(_)) => {
                            error!(
                                event = "audio_rtp_queue_closed",
                                "Audio sender task stopped"
                            );
                            return Err(gst::FlowError::Error);
                        }
                    }

                    Ok(gst::FlowSuccess::Ok)
                })
                .build(),
        );

        // Spawn async task to send RTP packets via WebRTC track
        let track_clone = Arc::clone(&track);
        let packet_count_clone = Arc::clone(&packet_count);
        let task_handle = tokio::spawn(async move {
            let parse_errors = RepeatLog::new("audio_rtp_parse_error", "invalid_packet");
            let send_errors = RepeatLog::new("audio_rtp_send_error", "track_write_failed");
            let mut first_timestamp: Option<u32> = None;
            let mut playback_start: Option<Instant> = None;

            while let Some(rtp_data) = rtp_rx.recv().await {
                // Parse RTP packet
                match RtpPacket::unmarshal(&mut rtp_data.as_ref()) {
                    Ok(rtp_packet) => {
                        let packet_timestamp = rtp_packet.header.timestamp;

                        match (first_timestamp, playback_start) {
                            (None, None) => {
                                first_timestamp = Some(packet_timestamp);
                                playback_start = Some(Instant::now());
                            }
                            (Some(base_timestamp), Some(base_instant)) => {
                                let elapsed_ticks =
                                    packet_timestamp.wrapping_sub(base_timestamp) as f64;
                                let target_instant = base_instant
                                    + Duration::from_secs_f64(elapsed_ticks / 48_000.0);
                                tokio::time::sleep_until(target_instant).await;
                            }
                            _ => {}
                        }

                        // Send RTP packet via WebRTC track
                        if track_clone.write_rtp(&rtp_packet).await.is_err() {
                            send_errors.record();
                        } else {
                            let sent = packet_count_clone.fetch_add(1, Ordering::Relaxed) + 1;

                            if sent.is_multiple_of(500) {
                                debug!(
                                    event = "audio_rtp_sent",
                                    packet_count = sent,
                                    "Sent RTP packets to WebRTC track"
                                );
                            }
                        }
                    }
                    Err(_) => parse_errors.record(),
                }
            }

            info!(
                event = "audio_sender_task_stop",
                total_packets = packet_count_clone.load(Ordering::Relaxed),
                "Audio sender task stopped"
            );
        });

        let bus_handle = if let Some(bus) = pipeline.bus() {
            let bus_running_clone = Arc::clone(&bus_running);
            let is_playing_clone = Arc::clone(&is_playing);
            let sender_abort = task_handle.abort_handle();
            let pipeline_clone = pipeline.clone();
            Some(thread::spawn(move || {
                let warnings = RepeatLog::new("audio_sender_gst_warning", "pipeline_warning");
                while bus_running_clone.load(Ordering::Relaxed) {
                    if let Some(msg) = bus.timed_pop(gst::ClockTime::from_mseconds(500)) {
                        use gst::MessageView;
                        match msg.view() {
                            MessageView::Error(err) => {
                                error!(
                                    event = "audio_sender_gst_error",
                                    error_code = err.error().code(),
                                    "Terminal audio pipeline failure; stopping playback"
                                );
                                is_playing_clone.store(false, Ordering::Relaxed);
                                bus_running_clone.store(false, Ordering::Relaxed);
                                sender_abort.abort();
                                if pipeline_clone.set_state(gst::State::Null).is_err() {
                                    error!(event = "audio_sender_shutdown_failed");
                                }
                                break;
                            }
                            MessageView::Warning(warning) => {
                                warnings.record_with(|count, suppressed_count| {
                                    warn!(
                                        event = "audio_sender_gst_warning",
                                        reason = "pipeline_warning",
                                        error_code = warning.error().code(),
                                        count,
                                        suppressed_count
                                    );
                                })
                            }
                            MessageView::Eos(..) => {
                                info!(
                                    event = "audio_sender_gst_eos",
                                    "GStreamer audio sender reached EOS"
                                );
                                break;
                            }
                            _ => {}
                        }
                    }
                }
            }))
        } else {
            None
        };

        Ok(Self {
            pipeline,
            is_playing,
            bus_running,
            packet_count,
            source_type: source,
            task_handle: Some(task_handle),
            bus_handle,
            rtp_tx: Some(rtp_tx),
            native_shutdown: false,
        })
    }

    pub fn play(&self) -> Result<(), String> {
        if !self.bus_running.load(Ordering::Relaxed) {
            return Err("Audio sender has stopped".into());
        }
        info!(
            event = "audio_sender_play",
            source_type = self.source_type.kind(),
            "Starting audio playback"
        );

        // Publish playing before state change so an asynchronous bus failure wins the race.
        self.is_playing.store(true, Ordering::Relaxed);
        if self.pipeline.set_state(gst::State::Playing).is_err() {
            self.is_playing.store(false, Ordering::Relaxed);
            return Err("Failed to set audio pipeline to Playing state".into());
        }
        if !self.bus_running.load(Ordering::Relaxed) {
            return Err("Audio sender failed during playback startup".into());
        }
        Ok(())
    }

    /// Synchronously stop the native pipeline and join its bus worker.
    /// Async session callers use owned shutdown so this never blocks Tokio.
    pub fn stop(&mut self) -> Result<(), String> {
        self.take_native_shutdown()
            .map_or(Ok(()), NativeAudioShutdown::stop)
    }

    fn take_native_shutdown(&mut self) -> Option<NativeAudioShutdown> {
        self.is_playing.store(false, Ordering::Relaxed);
        self.bus_running.store(false, Ordering::Relaxed);
        drop(self.rtp_tx.take());
        if let Some(task) = &self.task_handle {
            task.abort();
        }
        if self.native_shutdown {
            return None;
        }
        self.native_shutdown = true;
        Some(NativeAudioShutdown {
            pipeline: self.pipeline.clone(),
            bus_handle: self.bus_handle.take(),
            packet_count: Arc::clone(&self.packet_count),
        })
    }

    pub(crate) async fn shutdown(mut self) -> Result<(), String> {
        let native = self.take_native_shutdown();
        let task = self.task_handle.take();
        complete_audio_shutdown(native, task).await
    }

    pub fn pause(&self) -> Result<(), String> {
        self.pipeline
            .set_state(gst::State::Paused)
            .map_err(|e| format!("Failed to pause pipeline: {e:?}"))?;

        self.is_playing.store(false, Ordering::Relaxed);

        Ok(())
    }

    pub fn is_playing(&self) -> bool {
        self.is_playing.load(Ordering::Relaxed)
    }
}

struct NativeAudioShutdown {
    pipeline: gst::Pipeline,
    bus_handle: Option<thread::JoinHandle<()>>,
    packet_count: Arc<AtomicU64>,
}

impl NativeAudioShutdown {
    fn stop(self) -> Result<(), String> {
        info!(event = "audio_sender_stop", "Stopping audio playback");
        let pipeline_result = self
            .pipeline
            .set_state(gst::State::Null)
            .map(|_| ())
            .map_err(|_| "Failed to stop audio pipeline".to_string());
        let result = join_audio_bus(pipeline_result, self.bus_handle);
        info!(
            event = "audio_sender_stop_stats",
            total_packets = self.packet_count.load(Ordering::Relaxed),
            "Audio sender stop stats"
        );
        result
    }
}

fn join_audio_bus(
    pipeline_result: Result<(), String>,
    bus_handle: Option<thread::JoinHandle<()>>,
) -> Result<(), String> {
    // A state failure must not skip joining the other owned worker.
    let bus_result = bus_handle.map_or(Ok(()), |handle| {
        handle
            .join()
            .map_err(|_| "Audio bus worker failed".to_string())
    });
    pipeline_result.and(bus_result)
}

async fn complete_audio_shutdown(
    native: Option<NativeAudioShutdown>,
    task: Option<tokio::task::JoinHandle<()>>,
) -> Result<(), String> {
    let native_result = match native {
        Some(native) => tokio::task::spawn_blocking(move || native.stop())
            .await
            .unwrap_or_else(|_| Err("Audio cleanup worker failed".into())),
        None => Ok(()),
    };
    let task_result = match task {
        Some(task) => match task.await {
            Ok(()) => Ok(()),
            Err(error) if error.is_cancelled() => Ok(()),
            Err(_) => Err("Audio RTP worker failed".to_string()),
        },
        None => Ok(()),
    };
    native_result.and(task_result)
}

impl Drop for AudioSender {
    fn drop(&mut self) {
        let native = self.take_native_shutdown();
        let task = self.task_handle.take();
        if native.is_none() && task.is_none() {
            return;
        }
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            // Final drop is best effort; the owner task still observes both joins.
            // Session operations retain this completion separately across cancellation.
            runtime.spawn(async move {
                if complete_audio_shutdown(native, task).await.is_err() {
                    error!(event = "audio_sender_drop_failed", "Audio cleanup failed");
                }
            });
        } else if let Some(native) = native {
            // Without a live runtime only native teardown can be synchronous.
            if native.stop().is_err() {
                error!(event = "audio_sender_drop_failed", "Audio cleanup failed");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_gstreamer_init() {
        assert!(ensure_gst_init().is_ok(), "GStreamer init failed");
    }
    #[tokio::test]
    async fn file_path_with_launch_syntax_is_data_not_pipeline_code() {
        let path =
            std::env::temp_dir().join(format!("unitree audio ! \"{}.wav", std::process::id()));
        std::fs::write(&path, b"").unwrap();
        let track = Arc::new(TrackLocalStaticRTP::new(
            webrtc::rtp_transceiver::rtp_codec::RTCRtpCodecCapability {
                mime_type: "audio/opus".into(),
                clock_rate: 48000,
                channels: 2,
                ..Default::default()
            },
            "audio".into(),
            "test".into(),
        ));
        let result = AudioSender::new(AudioSource::File(path.to_str().unwrap().into()), track);
        std::fs::remove_file(path).unwrap();
        let mut sender = result.unwrap_or_else(|error| panic!("{error}"));
        sender.stop().unwrap();
    }

    #[tokio::test]
    async fn pipeline_error_stops_playback_and_sender_task() {
        let path =
            std::env::temp_dir().join(format!("unitree-fatal-audio-{}.wav", std::process::id()));
        std::fs::write(&path, b"").unwrap();
        let track = Arc::new(TrackLocalStaticRTP::new(
            webrtc::rtp_transceiver::rtp_codec::RTCRtpCodecCapability {
                mime_type: "audio/opus".into(),
                clock_rate: 48000,
                channels: 2,
                ..Default::default()
            },
            "audio".into(),
            "offline".into(),
        ));
        let sender =
            AudioSender::new(AudioSource::File(path.to_str().unwrap().into()), track).unwrap();
        std::fs::remove_file(path).unwrap();
        sender.is_playing.store(true, Ordering::Relaxed);
        sender
            .pipeline
            .bus()
            .unwrap()
            .post(
                gst::message::Error::builder(
                    gst::CoreError::Failed,
                    "secret-canary-pipeline-error",
                )
                .build(),
            )
            .unwrap();
        tokio::time::timeout(Duration::from_secs(2), async {
            while sender.is_playing() || !sender.task_handle.as_ref().unwrap().is_finished() {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("fatal pipeline error must stop playback and sender task");
        assert!(!sender.bus_running.load(Ordering::Relaxed));
        assert!(
            sender.play().is_err(),
            "a failed sender cannot silently resume an aborted task"
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn sender_final_drop_keeps_the_executor_live_and_finishes_workers() {
        let delayed = test_support::delayed_audio().await;
        let timer = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(5)).await;
            let _ = delayed.release.send(());
        });
        drop(delayed.sender);
        timer.await.unwrap();
        tokio::time::timeout(Duration::from_secs(2), async {
            while !delayed.native_joined.load(Ordering::SeqCst)
                || !delayed.rtp_completed.load(Ordering::SeqCst)
            {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("final audio drop must finish both owned workers");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn pipeline_stop_failure_still_joins_the_native_bus() {
        let (release, resume) = std::sync::mpsc::channel();
        let joined = Arc::new(AtomicBool::new(false));
        let completed = Arc::clone(&joined);
        let bus = thread::spawn(move || {
            if resume.recv_timeout(Duration::from_secs(1)).is_ok() {
                completed.store(true, Ordering::SeqCst);
            }
        });
        let (started, ready) = tokio::sync::oneshot::channel();
        let mut cleanup = tokio::task::spawn_blocking(move || {
            started.send(()).unwrap();
            join_audio_bus(Err("controlled pipeline stop failure".into()), Some(bus))
        });
        ready.await.unwrap();
        assert!(
            tokio::time::timeout(Duration::from_millis(30), &mut cleanup)
                .await
                .is_err(),
            "pipeline failure skipped the native join"
        );
        release.send(()).unwrap();
        assert!(cleanup.await.unwrap().is_err());
        assert!(joined.load(Ordering::SeqCst));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn shutdown_does_not_return_before_the_aborted_rtp_worker_completes() {
        let mut delayed = test_support::delayed_audio().await;
        let old = delayed.sender.task_handle.take().unwrap();
        old.abort();
        old.await.unwrap_err();
        let (release, resume) = std::sync::mpsc::channel();
        let (started, ready) = tokio::sync::oneshot::channel();
        let completed = Arc::new(AtomicBool::new(false));
        let worker_completed = Arc::clone(&completed);
        // A started blocking handle cannot be forcibly canceled. This controlled
        // worker makes dropping an aborted JoinHandle observably wrong.
        delayed.sender.task_handle = Some(tokio::task::spawn_blocking(move || {
            started.send(()).unwrap();
            if resume.recv_timeout(Duration::from_secs(1)).is_ok() {
                worker_completed.store(true, Ordering::SeqCst);
            }
        }));
        ready.await.unwrap();
        delayed.release.send(()).unwrap();
        let mut cleanup = tokio::spawn(delayed.sender.shutdown());
        assert!(
            tokio::time::timeout(Duration::from_millis(30), &mut cleanup)
                .await
                .is_err(),
            "shutdown detached the aborted RTP worker"
        );
        release.send(()).unwrap();
        cleanup.await.unwrap().unwrap();
        assert!(completed.load(Ordering::SeqCst));
        assert!(delayed.native_joined.load(Ordering::SeqCst));
    }

    #[test]
    fn source_rejects_invalid_and_unavailable_uri_schemes() {
        ensure_gst_init().unwrap();
        for url in [
            "relative/path",
            "https://example.com/\0secret",
            "unitree-unknown://host/a",
        ] {
            assert!(AudioSource::Url(url.into()).to_uri().is_err());
        }
        assert!(
            AudioSource::File(std::env::temp_dir().to_str().unwrap().into())
                .to_uri()
                .is_err()
        );
    }
}

#[cfg(test)]
pub(crate) mod test_support {
    use super::*;

    pub(crate) struct DelayedAudio {
        pub sender: AudioSender,
        pub release: std::sync::mpsc::Sender<()>,
        pub native_joined: Arc<AtomicBool>,
        pub rtp_completed: Arc<AtomicBool>,
        pub bus_running: Arc<AtomicBool>,
    }

    pub(crate) async fn delayed_audio() -> DelayedAudio {
        ensure_gst_init().unwrap();
        let (release, resume) = std::sync::mpsc::channel();
        let native_joined = Arc::new(AtomicBool::new(false));
        let joined = Arc::clone(&native_joined);
        let bus_handle = thread::spawn(move || {
            // A watchdog bounds broken synchronous cleanup; success requires the
            // Tokio timer/explicit test barrier to release this worker first.
            joined.store(
                resume.recv_timeout(Duration::from_secs(1)).is_ok(),
                Ordering::SeqCst,
            );
        });
        struct Complete(Arc<AtomicBool>);
        impl Drop for Complete {
            fn drop(&mut self) {
                self.0.store(true, Ordering::SeqCst);
            }
        }
        let rtp_completed = Arc::new(AtomicBool::new(false));
        let completed = Complete(Arc::clone(&rtp_completed));
        let (started, ready) = tokio::sync::oneshot::channel();
        let task_handle = tokio::spawn(async move {
            let _completed = completed;
            started.send(()).unwrap();
            std::future::pending::<()>().await;
        });
        ready.await.unwrap();
        let bus_running = Arc::new(AtomicBool::new(true));
        let sender = AudioSender {
            pipeline: gst::Pipeline::new(),
            is_playing: Arc::new(AtomicBool::new(true)),
            bus_running: Arc::clone(&bus_running),
            packet_count: Arc::new(AtomicU64::new(0)),
            source_type: AudioSource::File("offline".into()),
            task_handle: Some(task_handle),
            bus_handle: Some(bus_handle),
            rtp_tx: None,
            native_shutdown: false,
        };
        DelayedAudio {
            sender,
            release,
            native_joined,
            rtp_completed,
            bus_running,
        }
    }
}

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
use tracing::{info, warn};
use webrtc::rtp::packet::Packet as RtpPacket;
use webrtc::track::track_local::track_local_static_rtp::TrackLocalStaticRTP;
use webrtc::track::track_local::TrackLocalWriter;
use webrtc::util::Unmarshal;

#[derive(Clone)]
pub enum AudioSource {
    File(String),
    Url(String),
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
}

// SAFETY: AudioSender is Send because:
// 1. GStreamer Pipeline is internally thread-safe via GObject refcounting
// 2. Arc<AtomicBool> and Arc<AtomicU64> are Send
// 3. AudioSource (enum of owned Strings) is Send
// 4. tokio::task::JoinHandle and thread::JoinHandle are both Send
// 5. mpsc::Sender is Send
// 6. This sender is used as a single-owner within PyAudioBridge (Arc<AsyncMutex<Option<AudioSender>>>)
//    and only one play operation can be active at a time due to the mutex guard
unsafe impl Send for AudioSender {}

impl AudioSender {
    /// Create a new audio sender with GStreamer pipeline
    /// Pipeline: uridecodebin → audioconvert → audioresample → opusenc → rtpopuspay → appsink
    pub fn new(source: AudioSource, track: Arc<TrackLocalStaticRTP>) -> Result<Self, String> {
        ensure_gst_init()?;

        let uri = match &source {
            AudioSource::File(path) => {
                let abs_path = std::path::Path::new(path)
                    .canonicalize()
                    .map_err(|e| format!("Failed to resolve file path {path}: {e}"))?;
                format!("file://{}", abs_path.display())
            }
            AudioSource::Url(url) => url.clone(),
        };

        info!(
            event = "audio_sender_init",
            backend = "gstreamer",
            source_type = match &source {
                AudioSource::File(_) => "file",
                AudioSource::Url(_) => "url",
            },
            uri = %uri,
            "Creating audio sender pipeline"
        );

        // Build GStreamer pipeline
        let pipeline_str = format!(
            "uridecodebin uri={uri} name=src \
             ! audioconvert \
             ! audioresample \
             ! audio/x-raw,rate=48000,channels=2,format=S16LE \
             ! opusenc \
             ! rtpopuspay \
             ! appsink name=sink emit-signals=false sync=true max-buffers=2 drop=true"
        );

        let pipeline = gst::parse::launch(&pipeline_str)
            .map_err(|e| format!("Failed to create GStreamer audio pipeline: {e}"))?
            .downcast::<gst::Pipeline>()
            .map_err(|_| "Pipeline element is not a Pipeline".to_string())?;

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
                    if let Err(error) = rtp_tx_clone.try_send(rtp_data) {
                        warn!(
                            event = "audio_rtp_queue_drop",
                            error = %error,
                            "Dropped audio RTP packet because sender queue is full or closed"
                        );
                    }

                    Ok(gst::FlowSuccess::Ok)
                })
                .build(),
        );

        // Spawn async task to send RTP packets via WebRTC track
        let track_clone = Arc::clone(&track);
        let packet_count_clone = Arc::clone(&packet_count);
        let task_handle = tokio::spawn(async move {
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
                        if let Err(e) = track_clone.write_rtp(&rtp_packet).await {
                            warn!(
                                event = "audio_rtp_send_error",
                                error = %e,
                                "Failed to send RTP packet to WebRTC track"
                            );
                        } else {
                            let sent = packet_count_clone.fetch_add(1, Ordering::Relaxed) + 1;

                            if sent.is_multiple_of(500) {
                                info!(
                                    event = "audio_rtp_sent",
                                    packet_count = sent,
                                    "Sent RTP packets to WebRTC track"
                                );
                            }
                        }
                    }
                    Err(e) => {
                        warn!(
                            event = "audio_rtp_parse_error",
                            error = %e,
                            "Failed to parse RTP packet from GStreamer"
                        );
                    }
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
            Some(thread::spawn(move || {
                while bus_running_clone.load(Ordering::Relaxed) {
                    if let Some(msg) = bus.timed_pop(gst::ClockTime::from_mseconds(500)) {
                        use gst::MessageView;
                        match msg.view() {
                            MessageView::Error(err) => {
                                warn!(
                                    event = "audio_sender_gst_error",
                                    error = %err.error(),
                                    debug = ?err.debug().map(|d| d.to_string()),
                                    src = ?msg.src().map(|s| s.path_string().to_string()),
                                    "GStreamer audio sender pipeline error"
                                );
                            }
                            MessageView::Warning(warn_msg) => {
                                warn!(
                                    event = "audio_sender_gst_warning",
                                    error = %warn_msg.error(),
                                    debug = ?warn_msg.debug().map(|d| d.to_string()),
                                    src = ?msg.src().map(|s| s.path_string().to_string()),
                                    "GStreamer audio sender pipeline warning"
                                );
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
        })
    }

    pub fn play(&self) -> Result<(), String> {
        info!(
            event = "audio_sender_play",
            source = match &self.source_type {
                AudioSource::File(p) => format!("file:{p}"),
                AudioSource::Url(u) => format!("url:{u}"),
            },
            "Starting audio playback"
        );

        self.pipeline
            .set_state(gst::State::Playing)
            .map_err(|e| format!("Failed to set pipeline to Playing state: {e:?}"))?;

        self.is_playing.store(true, Ordering::Relaxed);

        Ok(())
    }

    pub fn stop(&mut self) -> Result<(), String> {
        info!(event = "audio_sender_stop", "Stopping audio playback");

        self.is_playing.store(false, Ordering::Relaxed);
        self.bus_running.store(false, Ordering::Relaxed);

        // Close RTP channel to stop async task
        drop(self.rtp_tx.take());

        // Abort async task if running
        if let Some(handle) = self.task_handle.take() {
            handle.abort();
        }

        self.pipeline
            .set_state(gst::State::Null)
            .map_err(|e| format!("Failed to stop pipeline: {e:?}"))?;

        if let Some(handle) = self.bus_handle.take() {
            let _ = handle.join();
        }

        info!(
            event = "audio_sender_stop_stats",
            total_packets = self.packet_count.load(Ordering::Relaxed),
            "Audio sender stop stats"
        );

        Ok(())
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

impl Drop for AudioSender {
    fn drop(&mut self) {
        self.bus_running.store(false, Ordering::Relaxed);
        if let Err(e) = self.pipeline.set_state(gst::State::Null) {
            warn!(
                event = "audio_sender_drop_failed",
                error = ?e,
                "Failed to stop audio sender pipeline on drop"
            );
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
}

use super::codec::OpusDecoder;
use crate::diagnostics::RepeatLog;
use crate::protocol::events::EventSender;
use crate::protocol::models::CallbackEvent;
use crossbeam_channel::TrySendError;
use std::sync::Arc;
use tracing::{debug, error, warn};
use webrtc::track::track_remote::TrackRemote;

pub fn spawn_audio_handler(
    track: Arc<TrackRemote>,
    callback_tx: EventSender,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        debug!(
            event = "audio_handler_start",
            ssrc = track.ssrc(),
            "Starting audio RTP handler"
        );

        let mut decoder = match OpusDecoder::new(48000, 2) {
            Ok(d) => d,
            Err(_) => {
                error!(
                    event = "audio_decoder_init_failed",
                    "Failed to initialize Opus decoder"
                );
                return;
            }
        };

        let frame_drops = RepeatLog::new("audio_frame_drop", "callback_queue_full");
        let decode_errors = RepeatLog::new("audio_decode_error", "invalid_opus_packet");
        let mut rtp_buffer = vec![0u8; 1500];
        let mut frame_count = 0u64;

        loop {
            match track.read(&mut rtp_buffer).await {
                Ok((rtp_packet, _attributes)) => {
                    // Opus RTP payload is directly the Opus packet (no fragmentation)
                    let opus_data = &rtp_packet.payload;

                    match decoder.decode(opus_data) {
                        Ok(decoded_audio) => {
                            if !decoded_audio.data.is_empty() {
                                frame_count += 1;

                                let event = CallbackEvent::AudioFrame {
                                    data: decoded_audio.data,
                                    sample_rate: decoded_audio.sample_rate,
                                    channels: decoded_audio.channels,
                                };

                                match callback_tx.try_send(event) {
                                    Ok(()) => {}
                                    Err(TrySendError::Full(_)) => frame_drops.record(),
                                    Err(TrySendError::Disconnected(_)) => {
                                        error!(
                                            event = "audio_callback_closed",
                                            "Stopping audio stream"
                                        );
                                        break;
                                    }
                                }
                            }
                        }
                        Err(_) => decode_errors.record(),
                    }
                }
                Err(_) => {
                    warn!(
                        event = "audio_rtp_read_error",
                        "Failed to read RTP packet from audio track"
                    );
                    break;
                }
            }
        }

        debug!(
            event = "audio_handler_stop",
            total_frames = frame_count,
            "Audio RTP handler stopped"
        );
    })
}

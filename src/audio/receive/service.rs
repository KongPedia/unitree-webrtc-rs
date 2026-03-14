use super::codec::OpusDecoder;
use crate::protocol::models::CallbackEvent;
use crossbeam_channel::Sender;
use std::sync::Arc;
use tracing::{info, warn};
use webrtc::track::track_remote::TrackRemote;

pub fn spawn_audio_handler(track: Arc<TrackRemote>, callback_tx: Sender<CallbackEvent>) {
    tokio::spawn(async move {
        info!(
            event = "audio_handler_start",
            ssrc = track.ssrc(),
            codec = %track.codec().capability.mime_type,
            "Starting audio RTP handler"
        );

        let mut decoder = match OpusDecoder::new(48000, 2) {
            Ok(d) => d,
            Err(e) => {
                warn!(
                    event = "audio_decoder_init_failed",
                    error = %e,
                    "Failed to initialize Opus decoder"
                );
                return;
            }
        };

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

                                if let Err(e) = callback_tx.try_send(event) {
                                    warn!(
                                        event = "audio_frame_drop",
                                        error = %e,
                                        "Failed to send audio frame to callback queue"
                                    );
                                }
                            }
                        }
                        Err(e) => {
                            warn!(
                                event = "audio_decode_error",
                                error = %e,
                                "Failed to decode Opus packet"
                            );
                        }
                    }
                }
                Err(e) => {
                    warn!(
                        event = "audio_rtp_read_error",
                        error = %e,
                        "Failed to read RTP packet from audio track"
                    );
                    break;
                }
            }
        }

        info!(
            event = "audio_handler_stop",
            total_frames = frame_count,
            "Audio RTP handler stopped"
        );
    });
}

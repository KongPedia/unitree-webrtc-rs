use super::codec::H264Decoder;
use super::h264_rtp::H264RtpReassembler;
use crate::diagnostics::RepeatLog;
use crate::protocol::events::EventSender;
use crate::protocol::models::CallbackEvent;
use crossbeam_channel::TrySendError;
use std::sync::Arc;
use tracing::{debug, error, warn};
use webrtc::track::track_remote::TrackRemote;

pub fn spawn_video_handler(
    track: Arc<TrackRemote>,
    callback_tx: EventSender,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        debug!(
            event = "video_handler_start",
            ssrc = track.ssrc(),
            "Starting video RTP handler"
        );

        let mut decoder = match H264Decoder::new() {
            Ok(d) => d,
            Err(_) => {
                error!(
                    event = "video_decoder_init_failed",
                    "Failed to initialize H264 decoder"
                );
                return;
            }
        };

        let mut reassembler = H264RtpReassembler::new();
        let mut frame_count = 0u64;
        let frame_drops = RepeatLog::new("video_frame_drop", "callback_queue_full");
        let mut rtp_buffer = vec![0u8; 1500];

        let mut health_poll = tokio::time::interval(std::time::Duration::from_millis(100));
        'stream: loop {
            let packet = {
                // Keep the same read alive across health polls: cancellation may
                // otherwise discard an RTP packet already consumed by the receiver.
                let read = track.read(&mut rtp_buffer);
                tokio::pin!(read);
                loop {
                    tokio::select! {
                        packet = &mut read => break packet,
                        _ = health_poll.tick() => {
                            if let Err(error) = decoder.check_health() {
                                error!(event = "video_decode_failed",
                                    error_class = %error,
                                    "Terminal H264 decoder failure; stopping video stream");
                                break 'stream;
                            }
                        }
                    }
                }
            };
            match packet {
                Ok((rtp_packet, _attributes)) => {
                    // Zero-copy: borrow payload directly instead of to_vec()
                    if let Some(nal_data) = reassembler.process_rtp_packet(
                        &rtp_packet.payload,
                        rtp_packet.header.sequence_number,
                        rtp_packet.header.timestamp,
                    ) {
                        match decoder.decode(&nal_data) {
                            Ok(Some(decoded_frame)) => {
                                frame_count += 1;

                                let event = CallbackEvent::VideoFrame {
                                    data: decoded_frame.data,
                                    width: decoded_frame.width,
                                    height: decoded_frame.height,
                                };

                                match callback_tx.try_send(event) {
                                    Ok(()) => {}
                                    Err(TrySendError::Full(_)) => frame_drops.record(),
                                    Err(TrySendError::Disconnected(_)) => {
                                        error!(
                                            event = "video_callback_closed",
                                            "Stopping video stream"
                                        );
                                        break;
                                    }
                                }
                            }
                            Ok(None) => {}
                            Err(error) => {
                                error!(
                                    event = "video_decode_failed",
                                    error_class = %error,
                                    "Terminal H264 decoder failure; stopping video stream"
                                );
                                break;
                            }
                        }
                    }
                }
                Err(_) => {
                    warn!(
                        event = "video_rtp_read_error",
                        "Failed to read RTP packet from video track"
                    );
                    break;
                }
            }
        }

        debug!(
            event = "video_handler_stop",
            ssrc = track.ssrc(),
            total_frames = frame_count,
            "Video RTP handler stopped"
        );
    })
}

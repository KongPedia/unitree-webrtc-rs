use crate::domain::models::CallbackEvent;
use crate::infrastructure::h264_rtp::H264RtpReassembler;
use crate::infrastructure::video_codec::H264Decoder;
use crossbeam_channel::Sender;
use std::sync::Arc;
use tracing::{info, warn};
use webrtc::track::track_remote::TrackRemote;

pub fn spawn_video_handler(track: Arc<TrackRemote>, callback_tx: Sender<CallbackEvent>) {
    tokio::spawn(async move {
        info!(
            event = "video_handler_start",
            ssrc = track.ssrc(),
            codec = %track.codec().capability.mime_type,
            "Starting video RTP handler"
        );

        let mut decoder = match H264Decoder::new() {
            Ok(d) => d,
            Err(e) => {
                warn!(
                    event = "video_decoder_init_failed",
                    error = %e,
                    "Failed to initialize H264 decoder"
                );
                return;
            }
        };

        let mut reassembler = H264RtpReassembler::new();
        let mut frame_count = 0u64;
        let mut rtp_buffer = vec![0u8; 1500];

        loop {
            match track.read(&mut rtp_buffer).await {
                Ok((rtp_packet, _attributes)) => {
                    // Zero-copy: borrow payload directly instead of to_vec()
                    if let Some(nal_data) = reassembler.process_packet(&rtp_packet.payload) {
                        match decoder.decode(&nal_data) {
                            Ok(Some(decoded_frame)) => {
                                frame_count += 1;

                                let event = CallbackEvent::VideoFrame {
                                    data: decoded_frame.data,
                                    width: decoded_frame.width,
                                    height: decoded_frame.height,
                                };

                                if let Err(e) = callback_tx.try_send(event) {
                                    warn!(
                                        event = "video_frame_drop",
                                        error = %e,
                                        "Failed to send video frame to callback queue"
                                    );
                                }
                            }
                            Ok(None) => {}
                            Err(_e) => {
                                // P-frames without SPS/PPS will fail - this is expected
                                // Only log at debug level to avoid noise
                            }
                        }
                    }
                }
                Err(e) => {
                    warn!(
                        event = "video_rtp_read_error",
                        error = %e,
                        "Failed to read RTP packet from video track"
                    );
                    break;
                }
            }
        }

        info!(
            event = "video_handler_stop",
            ssrc = track.ssrc(),
            total_frames = frame_count,
            "Video RTP handler stopped"
        );
    });
}

use crate::infrastructure::rtc_engine::state::RtcReadyState;
use crate::protocol::models::DcMessage;
use crossbeam_channel::{Sender, TrySendError};
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Arc;
use tracing::{info, warn};
use webrtc::data_channel::data_channel_message::DataChannelMessage;
use webrtc::data_channel::RTCDataChannel;
use webrtc::ice_transport::ice_connection_state::RTCIceConnectionState;
use webrtc::ice_transport::ice_gatherer_state::RTCIceGathererState;
use webrtc::peer_connection::peer_connection_state::RTCPeerConnectionState;
use webrtc::peer_connection::signaling_state::RTCSignalingState;
use webrtc::peer_connection::RTCPeerConnection;

pub fn bind_data_channel_handlers(
    data_channel: Arc<RTCDataChannel>,
    ready_state: Arc<AtomicU8>,
    incoming_tx: Sender<DcMessage>,
) {
    let open_state = Arc::clone(&ready_state);
    data_channel.on_open(Box::new(move || {
        let open_state = Arc::clone(&open_state);
        Box::pin(async move {
            open_state.store(RtcReadyState::Open as u8, Ordering::Relaxed);
            print_status("Data Channel", "open");
        })
    }));

    let close_state = Arc::clone(&ready_state);
    data_channel.on_close(Box::new(move || {
        let close_state = Arc::clone(&close_state);
        Box::pin(async move {
            close_state.store(RtcReadyState::Closed as u8, Ordering::Relaxed);
        })
    }));

    data_channel.on_message(Box::new(move |message: DataChannelMessage| {
        let incoming_tx = incoming_tx.clone();
        Box::pin(async move {
            if message.is_string {
                if let Ok(text) = String::from_utf8(message.data.to_vec()) {
                    if let Err(error) = incoming_tx.try_send(DcMessage::Text(text)) {
                        handle_send_error(error, "text");
                    }
                }
            } else if let Err(error) = incoming_tx.try_send(DcMessage::Binary(message.data)) {
                handle_send_error(error, "binary");
            }
        })
    }));
}

fn handle_send_error(error: TrySendError<DcMessage>, payload_kind: &str) {
    match error {
        TrySendError::Full(_) => {
            warn!(
                event = "dc_incoming_drop",
                payload_kind = payload_kind,
                reason = "incoming_queue_full",
                "Dropped datachannel payload due to backpressure"
            );
        }
        TrySendError::Disconnected(_) => {
            warn!(
                event = "dc_incoming_drop",
                payload_kind = payload_kind,
                reason = "incoming_queue_disconnected",
                "Dropped datachannel payload because receiver is unavailable"
            );
        }
    }
}

pub fn bind_peer_connection_handlers(peer_connection: Arc<RTCPeerConnection>) {
    peer_connection.on_ice_gathering_state_change(Box::new(move |state: RTCIceGathererState| {
        Box::pin(async move {
            match state {
                RTCIceGathererState::New => print_status("ICE Gathering", "new"),
                RTCIceGathererState::Gathering => print_status("ICE Gathering", "gathering"),
                RTCIceGathererState::Complete => print_status("ICE Gathering", "complete"),
                _ => {}
            }
        })
    }));

    peer_connection.on_ice_connection_state_change(Box::new(
        move |state: RTCIceConnectionState| {
            Box::pin(async move {
                match state {
                    RTCIceConnectionState::Checking => print_status("ICE Connection", "checking"),
                    RTCIceConnectionState::Completed => print_status("ICE Connection", "completed"),
                    RTCIceConnectionState::Failed => print_status("ICE Connection", "failed"),
                    RTCIceConnectionState::Closed => print_status("ICE Connection", "closed"),
                    _ => {}
                }
            })
        },
    ));

    peer_connection.on_peer_connection_state_change(Box::new(
        move |state: RTCPeerConnectionState| {
            Box::pin(async move {
                match state {
                    RTCPeerConnectionState::Connecting => {
                        print_status("Peer Connection", "connecting")
                    }
                    RTCPeerConnectionState::Connected => {
                        print_status("Peer Connection", "connected")
                    }
                    RTCPeerConnectionState::Closed => print_status("Peer Connection", "closed"),
                    RTCPeerConnectionState::Failed => print_status("Peer Connection", "failed"),
                    _ => {}
                }
            })
        },
    ));

    peer_connection.on_signaling_state_change(Box::new(move |state: RTCSignalingState| {
        Box::pin(async move {
            match state {
                RTCSignalingState::HaveLocalOffer => print_status("Signaling", "have_local_offer"),
                RTCSignalingState::HaveRemoteOffer => {
                    print_status("Signaling", "have_remote_offer")
                }
                RTCSignalingState::Stable => print_status("Signaling", "stable"),
                RTCSignalingState::Closed => print_status("Signaling", "closed"),
                _ => {}
            }
        })
    }));
}

fn print_status(status_type: &str, status_message: &str) {
    info!(
        event = "rtc_state_transition",
        component = status_type,
        state = status_message,
        "RTC state update"
    );
}

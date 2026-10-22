use crate::diagnostics::RepeatLog;
use crate::infrastructure::rtc_engine::state::{apply_event, RtcReadyState, RtcStateEvent};
use crate::protocol::models::{DcMessage, IngressMessage};
use crossbeam_channel::{Sender, TrySendError};
use std::sync::atomic::{AtomicU64, AtomicU8, Ordering};
use std::sync::Arc;
use tracing::{debug, warn};
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
    incoming_tx: Sender<IngressMessage>,
    generation: u64,
    current_generation: Arc<AtomicU64>,
) {
    let full = Arc::new(RepeatLog::new("dc_incoming_drop", "incoming_queue_full"));
    let closed = Arc::new(RepeatLog::new(
        "dc_incoming_drop",
        "incoming_queue_disconnected",
    ));
    let invalid_text = Arc::new(RepeatLog::new("datachannel_invalid_utf8", "invalid_utf8"));
    let open_state = Arc::clone(&ready_state);
    let open_generation = Arc::clone(&current_generation);
    data_channel.on_open(Box::new(move || {
        let open_state = Arc::clone(&open_state);
        let current = Arc::clone(&open_generation);
        Box::pin(async move {
            if generation != current.load(Ordering::SeqCst) {
                return;
            }
            apply_event(&open_state, RtcStateEvent::ChannelOpened);
            print_status("Data Channel", "open");
        })
    }));

    let close_state = Arc::clone(&ready_state);
    let close_generation = Arc::clone(&current_generation);
    data_channel.on_close(Box::new(move || {
        let close_state = Arc::clone(&close_state);
        let current = Arc::clone(&close_generation);
        Box::pin(async move {
            if generation != current.load(Ordering::SeqCst) {
                return;
            }
            apply_event(&close_state, RtcStateEvent::ChannelClosed);
        })
    }));

    data_channel.on_message(Box::new(move |message: DataChannelMessage| {
        let incoming_tx = incoming_tx.clone();
        let current_generation = Arc::clone(&current_generation);
        let ready_state = Arc::clone(&ready_state);
        let full = Arc::clone(&full);
        let closed = Arc::clone(&closed);
        let invalid_text = Arc::clone(&invalid_text);
        Box::pin(async move {
            if matches!(
                RtcReadyState::from_u8(ready_state.load(Ordering::SeqCst)),
                RtcReadyState::Closed | RtcReadyState::Failed
            ) {
                return;
            }
            if generation != current_generation.load(Ordering::SeqCst) {
                return;
            }
            if message.is_string {
                if let Some(text) = decode_text_message(&message.data) {
                    if let Err(error) = enqueue_current(
                        &incoming_tx,
                        generation,
                        &current_generation,
                        DcMessage::Text(text),
                    ) {
                        handle_send_error(error, &full, &closed);
                    }
                } else {
                    invalid_text.record();
                }
            } else if let Err(error) = enqueue_current(
                &incoming_tx,
                generation,
                &current_generation,
                DcMessage::Binary(message.data),
            ) {
                handle_send_error(error, &full, &closed);
            }
        })
    }));
}

fn enqueue_current(
    incoming: &Sender<IngressMessage>,
    generation: u64,
    current: &AtomicU64,
    payload: DcMessage,
) -> Result<(), TrySendError<IngressMessage>> {
    if generation != current.load(Ordering::SeqCst) {
        return Ok(());
    }
    incoming.try_send(payload.for_generation(generation))
}

fn handle_send_error(error: TrySendError<IngressMessage>, full: &RepeatLog, closed: &RepeatLog) {
    match error {
        TrySendError::Full(_) => full.record(),
        TrySendError::Disconnected(_) => closed.record(),
    }
}

pub fn bind_peer_connection_handlers(
    peer_connection: Arc<RTCPeerConnection>,
    ready_state: Arc<AtomicU8>,
    generation: u64,
    current_generation: Arc<AtomicU64>,
) {
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
            let ready_state = Arc::clone(&ready_state);
            let current = Arc::clone(&current_generation);
            Box::pin(async move {
                if generation != current.load(Ordering::SeqCst) {
                    return;
                }
                let event = match state {
                    RTCPeerConnectionState::Connected => Some(RtcStateEvent::PeerConnected),
                    RTCPeerConnectionState::Disconnected => Some(RtcStateEvent::PeerDisconnected),
                    RTCPeerConnectionState::Failed => Some(RtcStateEvent::PeerFailed),
                    RTCPeerConnectionState::Closed => Some(RtcStateEvent::PeerClosed),
                    _ => None,
                };
                if let Some(event) = event {
                    apply_event(&ready_state, event);
                }
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

fn decode_text_message(data: &[u8]) -> Option<String> {
    std::str::from_utf8(data).ok().map(str::to_owned)
}

fn print_status(status_type: &str, status_message: &str) {
    if status_message == "failed" {
        warn!(
            event = "rtc_state_transition",
            component = status_type,
            state = status_message,
            "RTC transport failed"
        );
        return;
    }
    debug!(
        event = "rtc_state_transition",
        component = status_type,
        state = status_message,
        "RTC state update"
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn datachannel_text_rejects_invalid_utf8_and_preserves_valid_text() {
        assert_eq!(decode_text_message(b"hello"), Some("hello".into()));
        assert_eq!(decode_text_message("로봇".as_bytes()), Some("로봇".into()));
        assert!(decode_text_message(b"sentinel-private-key\xff").is_none());
    }

    #[test]
    fn peer_ingress_preserves_captured_generation_and_drops_stale_producers() {
        let (sender, receiver) = crossbeam_channel::bounded(4);
        let current = AtomicU64::new(7);
        enqueue_current(&sender, 7, &current, DcMessage::Text("first".into())).unwrap();
        current.store(8, Ordering::SeqCst);
        enqueue_current(&sender, 7, &current, DcMessage::Text("late".into())).unwrap();
        enqueue_current(&sender, 7, &current, DcMessage::Binary(vec![1].into())).unwrap();
        enqueue_current(&sender, 8, &current, DcMessage::Binary(vec![2].into())).unwrap();
        assert_eq!(
            receiver.recv().unwrap(),
            DcMessage::Text("first".into()).for_generation(7)
        );
        assert_eq!(
            receiver.recv().unwrap(),
            DcMessage::Binary(vec![2].into()).for_generation(8)
        );
        assert!(receiver.try_recv().is_err());
    }
}

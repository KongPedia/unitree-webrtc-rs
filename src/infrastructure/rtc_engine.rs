use crate::application::audio_service::spawn_audio_handler;
use crate::application::video_service::spawn_video_handler;
use crate::domain::models::{CallbackEvent, DcMessage};
use crate::domain::ports::{DataChannelPort, PortResult, RtcEnginePort};
use bytes::Bytes;
use crossbeam_channel::{Sender, TrySendError};
use serde_json::Value;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Arc;
use std::sync::Mutex as StdMutex;
use std::time::{Duration, Instant};
use tokio::sync::mpsc::{unbounded_channel, UnboundedSender};
use tokio::sync::Mutex;
use tracing::{info, warn};
use webrtc::api::interceptor_registry::register_default_interceptors;
use webrtc::api::media_engine::MediaEngine;
use webrtc::api::APIBuilder;
use webrtc::data_channel::data_channel_message::DataChannelMessage;
use webrtc::data_channel::RTCDataChannel;
use webrtc::ice_transport::ice_connection_state::RTCIceConnectionState;
use webrtc::ice_transport::ice_gatherer_state::RTCIceGathererState;
use webrtc::interceptor::registry::Registry;
use webrtc::peer_connection::configuration::RTCConfiguration;
use webrtc::peer_connection::peer_connection_state::RTCPeerConnectionState;
use webrtc::peer_connection::sdp::session_description::RTCSessionDescription;
use webrtc::peer_connection::signaling_state::RTCSignalingState;
use webrtc::peer_connection::RTCPeerConnection;
use webrtc::rtp_transceiver::rtp_codec::RTCRtpCodecCapability;
use webrtc::rtp_transceiver::rtp_codec::RTPCodecType;
use webrtc::rtp_transceiver::rtp_transceiver_direction::RTCRtpTransceiverDirection;
use webrtc::rtp_transceiver::RTCRtpTransceiverInit;
use webrtc::track::track_local::track_local_static_rtp::TrackLocalStaticRTP;
use webrtc::track::track_local::TrackLocal;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum RtcReadyState {
    Closed = 0,
    Connecting = 1,
    Open = 2,
}

impl RtcReadyState {
    fn from_u8(value: u8) -> Self {
        match value {
            1 => Self::Connecting,
            2 => Self::Open,
            _ => Self::Closed,
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Closed => "closed",
            Self::Connecting => "connecting",
            Self::Open => "open",
        }
    }
}

#[derive(Debug, Clone)]
enum EngineCommand {
    Text(String),
    Binary(Vec<u8>),
}

#[derive(Clone)]
pub struct RtcEngine {
    incoming_tx: Sender<DcMessage>,
    callback_tx: Sender<CallbackEvent>,
    ready_state: Arc<AtomicU8>,
    peer_connection: Arc<Mutex<Option<Arc<RTCPeerConnection>>>>,
    data_channel: Arc<Mutex<Option<Arc<RTCDataChannel>>>>,
    command_tx: Arc<StdMutex<Option<UnboundedSender<EngineCommand>>>>,
    audio_track: Arc<Mutex<Option<Arc<TrackLocalStaticRTP>>>>,
}

impl RtcEngine {
    pub fn new(incoming_tx: Sender<DcMessage>, callback_tx: Sender<CallbackEvent>) -> Self {
        Self {
            incoming_tx,
            callback_tx,
            ready_state: Arc::new(AtomicU8::new(RtcReadyState::Closed as u8)),
            peer_connection: Arc::new(Mutex::new(None)),
            data_channel: Arc::new(Mutex::new(None)),
            command_tx: Arc::new(StdMutex::new(None)),
            audio_track: Arc::new(Mutex::new(None)),
        }
    }

    pub async fn prepare_offer(&self) -> PortResult<String> {
        self.set_ready_state(RtcReadyState::Connecting);

        let mut media_engine = MediaEngine::default();
        media_engine
            .register_default_codecs()
            .map_err(|error| error.to_string())?;

        let mut interceptor_registry = Registry::new();
        interceptor_registry =
            register_default_interceptors(interceptor_registry, &mut media_engine)
                .map_err(|error| error.to_string())?;

        let api = APIBuilder::new()
            .with_media_engine(media_engine)
            .with_interceptor_registry(interceptor_registry)
            .build();

        let peer_connection = Arc::new(
            api.new_peer_connection(RTCConfiguration::default())
                .await
                .map_err(|error| error.to_string())?,
        );
        bind_peer_connection_handlers(Arc::clone(&peer_connection));

        // Add video transceiver (recvonly - we only receive video from Go2)
        let video_init = RTCRtpTransceiverInit {
            direction: RTCRtpTransceiverDirection::Recvonly,
            send_encodings: vec![],
        };
        peer_connection
            .add_transceiver_from_kind(RTPCodecType::Video, Some(video_init))
            .await
            .map_err(|error| error.to_string())?;

        // Create audio track for transmission (PC → Robot)
        // Note: add_track automatically creates a transceiver, so we don't need add_transceiver_from_kind
        let audio_track = Arc::new(TrackLocalStaticRTP::new(
            RTCRtpCodecCapability {
                mime_type: "audio/opus".to_owned(),
                clock_rate: 48000,
                channels: 2,
                sdp_fmtp_line: "minptime=10;useinbandfec=1".to_owned(),
                rtcp_feedback: vec![],
            },
            "audio".to_owned(),
            "unitree-webrtc-audio".to_owned(),
        ));

        // Add audio track to peer connection
        let _rtp_sender = peer_connection
            .add_track(Arc::clone(&audio_track) as Arc<dyn TrackLocal + Send + Sync>)
            .await
            .map_err(|error| format!("Failed to add audio track: {error}"))?;

        // Store audio track for later use
        *self.audio_track.lock().await = Some(audio_track);

        info!(
            event = "audio_track_created",
            codec = "opus",
            sample_rate = 48000,
            channels = 2,
            "Created audio track for transmission"
        );

        // Register on_track handler for incoming video/audio tracks
        let callback_tx_for_track = self.callback_tx.clone();
        peer_connection.on_track(Box::new(move |track, _receiver, _transceiver| {
            let callback_tx = callback_tx_for_track.clone();
            Box::pin(async move {
                let codec_name = track.codec().capability.mime_type.clone();
                let track_kind = track.kind();

                info!(
                    event = "track_received",
                    kind = %track_kind,
                    codec = %codec_name,
                    ssrc = track.ssrc(),
                    "Received media track from peer"
                );

                match track_kind {
                    RTPCodecType::Video => {
                        spawn_video_handler(track, callback_tx);
                    }
                    RTPCodecType::Audio => {
                        spawn_audio_handler(track, callback_tx);
                    }
                    _ => {
                        warn!(
                            event = "track_kind_unhandled",
                            kind = %track_kind,
                            "Received track of unknown kind"
                        );
                    }
                }
            })
        }));

        let ready_state = Arc::clone(&self.ready_state);
        let incoming_tx = self.incoming_tx.clone();
        peer_connection.on_data_channel(Box::new(move |dc: Arc<RTCDataChannel>| {
            let ready_state = Arc::clone(&ready_state);
            let incoming_tx = incoming_tx.clone();
            Box::pin(async move {
                bind_data_channel_handlers(dc, ready_state, incoming_tx);
            })
        }));

        let data_channel = peer_connection
            .create_data_channel("data", None)
            .await
            .map_err(|error| error.to_string())?;
        bind_data_channel_handlers(
            Arc::clone(&data_channel),
            Arc::clone(&self.ready_state),
            self.incoming_tx.clone(),
        );

        {
            let mut peer_connection_guard = self.peer_connection.lock().await;
            *peer_connection_guard = Some(Arc::clone(&peer_connection));
        }
        {
            let mut data_channel_guard = self.data_channel.lock().await;
            *data_channel_guard = Some(Arc::clone(&data_channel));
        }

        let (command_tx, mut command_rx) = unbounded_channel::<EngineCommand>();
        {
            let mut command_guard = self.command_tx.lock().unwrap();
            *command_guard = Some(command_tx);
        }

        let data_channel_for_send = Arc::clone(&data_channel);
        tokio::spawn(async move {
            while let Some(command) = command_rx.recv().await {
                match command {
                    EngineCommand::Text(text) => {
                        let _ = data_channel_for_send.send_text(text).await;
                    }
                    EngineCommand::Binary(binary) => {
                        let _ = data_channel_for_send.send(&Bytes::from(binary)).await;
                    }
                }
            }
        });

        let mut gather_complete = peer_connection.gathering_complete_promise().await;
        let offer = peer_connection
            .create_offer(None)
            .await
            .map_err(|error| error.to_string())?;

        peer_connection
            .set_local_description(offer)
            .await
            .map_err(|error| error.to_string())?;
        let _ = gather_complete.recv().await;

        let local_description = peer_connection
            .local_description()
            .await
            .ok_or_else(|| "Missing local SDP description".to_string())?;

        tracing::debug!(
            event = "sdp_offer_full",
            sdp = %local_description.sdp,
            "Generated full SDP offer"
        );

        info!(
            event = "sdp_offer_generated",
            sdp_lines = local_description.sdp.lines().count(),
            "Generated SDP offer"
        );

        Ok(local_description.sdp)
    }

    pub async fn apply_answer(&self, answer_payload: &str) -> PortResult<()> {
        let peer_connection = {
            let peer_connection_guard = self.peer_connection.lock().await;
            peer_connection_guard
                .clone()
                .ok_or_else(|| "Peer connection is not initialized".to_string())?
        };

        let answer = parse_answer_payload(answer_payload)?;
        peer_connection
            .set_remote_description(answer)
            .await
            .map_err(|error| error.to_string())?;

        let timeout = Duration::from_secs(30);
        let started_at = Instant::now();
        loop {
            if self.current_ready_state() == RtcReadyState::Open {
                return Ok(());
            }

            if started_at.elapsed() > timeout {
                return Err("Data channel did not open in time".to_string());
            }

            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    pub async fn close(&self) {
        {
            let mut command_guard = self.command_tx.lock().unwrap();
            *command_guard = None;
        }

        let data_channel = {
            let mut data_channel_guard = self.data_channel.lock().await;
            data_channel_guard.take()
        };
        if let Some(channel) = data_channel {
            let _ = channel.close().await;
        }

        let peer_connection = {
            let mut peer_connection_guard = self.peer_connection.lock().await;
            peer_connection_guard.take()
        };
        if let Some(connection) = peer_connection {
            let _ = connection.close().await;
        }

        self.set_ready_state(RtcReadyState::Closed);
    }

    pub fn route_text_message(&self, payload: String) -> PortResult<()> {
        self.incoming_tx
            .try_send(DcMessage::Text(payload))
            .map_err(|error| error.to_string())
    }

    pub fn route_binary_message(&self, payload: Vec<u8>) -> PortResult<()> {
        self.incoming_tx
            .try_send(DcMessage::Binary(payload))
            .map_err(|error| error.to_string())
    }

    pub fn current_ready_state(&self) -> RtcReadyState {
        RtcReadyState::from_u8(self.ready_state.load(Ordering::Relaxed))
    }

    fn set_ready_state(&self, state: RtcReadyState) {
        self.ready_state.store(state as u8, Ordering::Relaxed);
    }

    pub async fn get_audio_track(&self) -> Option<Arc<TrackLocalStaticRTP>> {
        self.audio_track.lock().await.clone()
    }
}

impl DataChannelPort for RtcEngine {
    fn send_text(&self, message: &str) -> PortResult<()> {
        if self.current_ready_state() != RtcReadyState::Open {
            return Err("DataChannel is not open".to_string());
        }

        let command_tx = {
            let guard = self.command_tx.lock().unwrap();
            guard.clone()
        }
        .ok_or_else(|| "DataChannel command sender is not initialized".to_string())?;

        command_tx
            .send(EngineCommand::Text(message.to_string()))
            .map_err(|error| error.to_string())
    }

    fn send_binary(&self, bytes: &[u8]) -> PortResult<()> {
        if self.current_ready_state() != RtcReadyState::Open {
            return Err("DataChannel is not open".to_string());
        }

        let command_tx = {
            let guard = self.command_tx.lock().unwrap();
            guard.clone()
        }
        .ok_or_else(|| "DataChannel command sender is not initialized".to_string())?;

        command_tx
            .send(EngineCommand::Binary(bytes.to_vec()))
            .map_err(|error| error.to_string())
    }

    fn set_message_sender(&self, _sender: Sender<DcMessage>) -> PortResult<()> {
        Err("RtcEngine sender is fixed at construction".to_string())
    }

    fn ready_state(&self) -> &'static str {
        self.current_ready_state().as_str()
    }
}

impl RtcEnginePort for RtcEngine {
    fn prepare_offer<'a>(
        &'a self,
    ) -> Pin<Box<dyn Future<Output = PortResult<String>> + Send + 'a>> {
        Box::pin(async move { RtcEngine::prepare_offer(self).await })
    }

    fn apply_answer<'a>(
        &'a self,
        answer_sdp: &'a str,
    ) -> Pin<Box<dyn Future<Output = PortResult<()>> + Send + 'a>> {
        Box::pin(async move { RtcEngine::apply_answer(self, answer_sdp).await })
    }

    fn close<'a>(&'a self) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>> {
        Box::pin(async move { RtcEngine::close(self).await })
    }
}

fn bind_data_channel_handlers(
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
                        match error {
                            TrySendError::Full(_) => {
                                warn!(
                                    event = "dc_incoming_drop",
                                    payload_kind = "text",
                                    reason = "incoming_queue_full",
                                    "Dropped text datachannel payload due to backpressure"
                                );
                            }
                            TrySendError::Disconnected(_) => {
                                warn!(
                                    event = "dc_incoming_drop",
                                    payload_kind = "text",
                                    reason = "incoming_queue_disconnected",
                                    "Dropped text datachannel payload because receiver is unavailable"
                                );
                            }
                        }
                    }
                }
            } else if let Err(error) = incoming_tx.try_send(DcMessage::Binary(message.data.to_vec()))
            {
                match error {
                    TrySendError::Full(_) => {
                        warn!(
                            event = "dc_incoming_drop",
                            payload_kind = "binary",
                            reason = "incoming_queue_full",
                            "Dropped binary datachannel payload due to backpressure"
                        );
                    }
                    TrySendError::Disconnected(_) => {
                        warn!(
                            event = "dc_incoming_drop",
                            payload_kind = "binary",
                            reason = "incoming_queue_disconnected",
                            "Dropped binary datachannel payload because receiver is unavailable"
                        );
                    }
                }
            }
        })
    }));
}

fn bind_peer_connection_handlers(peer_connection: Arc<RTCPeerConnection>) {
    peer_connection.on_ice_gathering_state_change(Box::new(move |state: RTCIceGathererState| {
        Box::pin(async move {
            match state {
                RTCIceGathererState::New => {
                    print_status("ICE Gathering", "new");
                }
                RTCIceGathererState::Gathering => {
                    print_status("ICE Gathering", "gathering");
                }
                RTCIceGathererState::Complete => {
                    print_status("ICE Gathering", "complete");
                }
                _ => {}
            }
        })
    }));

    peer_connection.on_ice_connection_state_change(Box::new(
        move |state: RTCIceConnectionState| {
            Box::pin(async move {
                match state {
                    RTCIceConnectionState::Checking => {
                        print_status("ICE Connection", "checking");
                    }
                    RTCIceConnectionState::Completed => {
                        print_status("ICE Connection", "completed");
                    }
                    RTCIceConnectionState::Failed => {
                        print_status("ICE Connection", "failed");
                    }
                    RTCIceConnectionState::Closed => {
                        print_status("ICE Connection", "closed");
                    }
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
                        print_status("Peer Connection", "connecting");
                    }
                    RTCPeerConnectionState::Connected => {
                        print_status("Peer Connection", "connected");
                    }
                    RTCPeerConnectionState::Closed => {
                        print_status("Peer Connection", "closed");
                    }
                    RTCPeerConnectionState::Failed => {
                        print_status("Peer Connection", "failed");
                    }
                    _ => {}
                }
            })
        },
    ));

    peer_connection.on_signaling_state_change(Box::new(move |state: RTCSignalingState| {
        Box::pin(async move {
            match state {
                RTCSignalingState::HaveLocalOffer => {
                    print_status("Signaling", "have_local_offer");
                }
                RTCSignalingState::HaveRemoteOffer => {
                    print_status("Signaling", "have_remote_offer");
                }
                RTCSignalingState::Stable => {
                    print_status("Signaling", "stable");
                }
                RTCSignalingState::Closed => {
                    print_status("Signaling", "closed");
                }
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

fn parse_answer_payload(answer_payload: &str) -> PortResult<RTCSessionDescription> {
    if let Ok(value) = serde_json::from_str::<Value>(answer_payload) {
        if value.get("sdp").and_then(Value::as_str) == Some("reject") {
            return Err(
                "Go2 is connected by another WebRTC client. Close your mobile APP and try again."
                    .to_string(),
            );
        }

        if let Ok(answer) = serde_json::from_value::<RTCSessionDescription>(value.clone()) {
            return Ok(answer);
        }

        if let Some(sdp) = value.get("sdp").and_then(Value::as_str) {
            return RTCSessionDescription::answer(sdp.to_string())
                .map_err(|error| error.to_string());
        }
    }

    RTCSessionDescription::answer(answer_payload.to_string()).map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossbeam_channel::bounded;

    #[test]
    fn routes_text_message_to_channel() {
        let (tx, rx) = bounded::<DcMessage>(4);
        let (callback_tx, _callback_rx) = bounded::<CallbackEvent>(4);
        let engine = RtcEngine::new(tx, callback_tx);

        engine.route_text_message("hello".to_string()).unwrap();

        assert_eq!(rx.recv().unwrap(), DcMessage::Text("hello".to_string()));
    }

    #[test]
    fn routes_binary_message_to_channel() {
        let (tx, rx) = bounded::<DcMessage>(4);
        let (callback_tx, _callback_rx) = bounded::<CallbackEvent>(4);
        let engine = RtcEngine::new(tx, callback_tx);

        engine.route_binary_message(vec![1, 2, 3]).unwrap();

        assert_eq!(rx.recv().unwrap(), DcMessage::Binary(vec![1, 2, 3]));
    }

    #[test]
    fn parse_answer_detects_reject() {
        let payload = r#"{\"type\":\"answer\",\"sdp\":\"reject\"}"#;
        let result = parse_answer_payload(payload);
        assert!(result.is_err());
    }
}

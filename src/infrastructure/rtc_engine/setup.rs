use crate::audio::spawn_audio_handler;
use crate::infrastructure::rtc_engine::handlers::{
    bind_data_channel_handlers, bind_peer_connection_handlers,
};
use crate::protocol::models::CallbackEvent;
use crate::protocol::ports::PortResult;
use crate::video::spawn_video_handler;
use crossbeam_channel::Sender;
use std::sync::atomic::AtomicU8;
use std::sync::Arc;
use tracing::{info, warn};
use webrtc::api::interceptor_registry::register_default_interceptors;
use webrtc::api::media_engine::MediaEngine;
use webrtc::api::APIBuilder;
use webrtc::data_channel::RTCDataChannel;
use webrtc::interceptor::registry::Registry;
use webrtc::peer_connection::configuration::RTCConfiguration;
use webrtc::peer_connection::RTCPeerConnection;
use webrtc::rtp_transceiver::rtp_codec::RTCRtpCodecCapability;
use webrtc::rtp_transceiver::rtp_codec::RTPCodecType;
use webrtc::rtp_transceiver::rtp_transceiver_direction::RTCRtpTransceiverDirection;
use webrtc::rtp_transceiver::RTCRtpTransceiverInit;
use webrtc::track::track_local::track_local_static_rtp::TrackLocalStaticRTP;
use webrtc::track::track_local::TrackLocal;

pub async fn create_peer_connection(
    incoming_tx: Sender<crate::protocol::models::DcMessage>,
    callback_tx: Sender<CallbackEvent>,
    ready_state: Arc<AtomicU8>,
) -> PortResult<(
    Arc<RTCPeerConnection>,
    Arc<RTCDataChannel>,
    Arc<TrackLocalStaticRTP>,
)> {
    // Create MediaEngine with default codecs
    let mut media_engine = MediaEngine::default();
    media_engine
        .register_default_codecs()
        .map_err(|error| error.to_string())?;

    // Setup interceptor registry
    let mut interceptor_registry = Registry::new();
    interceptor_registry = register_default_interceptors(interceptor_registry, &mut media_engine)
        .map_err(|error| error.to_string())?;

    // Build WebRTC API
    let api = APIBuilder::new()
        .with_media_engine(media_engine)
        .with_interceptor_registry(interceptor_registry)
        .build();

    // Create peer connection
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

    info!(
        event = "audio_track_created",
        codec = "opus",
        sample_rate = 48000,
        channels = 2,
        "Created audio track for transmission"
    );

    // Register on_track handler for incoming media
    let callback_tx_for_track = callback_tx.clone();
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

    // Setup data channel handlers
    let ready_state_clone = Arc::clone(&ready_state);
    let incoming_tx_clone = incoming_tx.clone();
    peer_connection.on_data_channel(Box::new(move |dc: Arc<RTCDataChannel>| {
        let ready_state = Arc::clone(&ready_state_clone);
        let incoming_tx = incoming_tx_clone.clone();
        Box::pin(async move {
            bind_data_channel_handlers(dc, ready_state, incoming_tx);
        })
    }));

    // Create data channel
    let data_channel = peer_connection
        .create_data_channel("data", None)
        .await
        .map_err(|error| error.to_string())?;

    bind_data_channel_handlers(
        Arc::clone(&data_channel),
        Arc::clone(&ready_state),
        incoming_tx,
    );

    Ok((peer_connection, data_channel, audio_track))
}

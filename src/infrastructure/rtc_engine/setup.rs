use crate::audio::spawn_audio_handler;
use crate::infrastructure::rtc_engine::handlers::{
    bind_data_channel_handlers, bind_peer_connection_handlers,
};
#[cfg(test)]
use crate::protocol::events::event_channel;
use crate::protocol::events::EventSender;
use crate::protocol::ports::PortResult;
use crate::video::spawn_video_handler;
use crossbeam_channel::Sender;
use std::sync::atomic::{AtomicU64, AtomicU8};
use std::sync::{Arc, Mutex};
use tokio::task::JoinHandle;
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

pub(crate) struct PeerResources {
    peer: Option<Arc<RTCPeerConnection>>,
    media: Arc<Mutex<Option<Vec<JoinHandle<()>>>>>,
    command: Option<tokio::task::AbortHandle>,
    #[cfg(test)]
    pub(super) cleanup_barrier: Option<(
        tokio::sync::oneshot::Sender<()>,
        tokio::sync::oneshot::Receiver<()>,
    )>,
}

impl PeerResources {
    fn new(peer: Arc<RTCPeerConnection>) -> Self {
        Self {
            peer: Some(peer),
            command: None,
            #[cfg(test)]
            cleanup_barrier: None,
            media: Arc::new(Mutex::new(Some(Vec::new()))),
        }
    }

    pub(super) fn register(
        peer: Arc<RTCPeerConnection>,
        owner: &Arc<Mutex<Option<Self>>>,
    ) -> Arc<Mutex<Option<Vec<JoinHandle<()>>>>> {
        let resources = Self::new(peer);
        let media = Arc::clone(&resources.media);
        *owner.lock().unwrap() = Some(resources);
        media
    }

    pub(crate) fn own_command(&mut self, task: tokio::task::AbortHandle) {
        self.command = Some(task);
    }

    fn abort_media(&self) {
        if let Some(command) = &self.command {
            command.abort();
        }
        if let Some(tasks) = self.media.lock().unwrap().take() {
            for task in tasks {
                task.abort();
            }
        }
    }

    pub(crate) async fn close(mut self) {
        #[cfg(test)]
        if let Some((started, resume)) = self.cleanup_barrier.take() {
            let _ = started.send(());
            let _ = resume.await;
        }
        let tasks = self.media.lock().unwrap().take().unwrap_or_default();
        for task in &tasks {
            task.abort();
        }
        for task in tasks {
            let _ = task.await;
        }
        if let Some(peer) = self.peer.take() {
            if peer.close().await.is_err() {
                warn!(
                    event = "peer_close_failed",
                    "Failed to close peer connection"
                );
            }
        }
    }
}

impl Drop for PeerResources {
    fn drop(&mut self) {
        self.abort_media();
        if let Some(peer) = self.peer.take() {
            // Setup runs inside Tokio; cancellation must still close its partial peer.
            if let Ok(runtime) = tokio::runtime::Handle::try_current() {
                runtime.spawn(async move {
                    if peer.close().await.is_err() {
                        warn!(
                            event = "partial_peer_close_failed",
                            "Failed to close partial peer connection"
                        );
                    }
                });
            }
        }
    }
}

pub(crate) async fn create_peer_connection(
    incoming_tx: Sender<crate::protocol::models::IngressMessage>,
    callback_tx: EventSender,
    ready_state: Arc<AtomicU8>,
    generation: u64,
    ingress_generation: Arc<AtomicU64>,
    resources_owner: Arc<Mutex<Option<PeerResources>>>,
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

    // Register before any further await so cancellation leaves tracked cleanup.
    let media_tasks = PeerResources::register(Arc::clone(&peer_connection), &resources_owner);
    bind_peer_connection_handlers(
        Arc::clone(&peer_connection),
        Arc::clone(&ready_state),
        generation,
        Arc::clone(&ingress_generation),
    );
    let result = async {
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

        // Keep upstream fmtp/payload definitions, but advertise only our implemented decoders.
        for transceiver in peer_connection.get_transceivers().await {
            let codecs = transceiver
                .receiver()
                .await
                .get_parameters()
                .await
                .codecs
                .into_iter()
                .filter(|codec| supported_codec(transceiver.kind(), &codec.capability))
                .collect::<Vec<_>>();
            if codecs.is_empty() {
                return Err("No implemented media codec available".to_string());
            }
            transceiver
                .set_codec_preferences(codecs)
                .await
                .map_err(|error| error.to_string())?;
        }

        // Register on_track handler for incoming media
        let callback_tx_for_track = callback_tx.for_generation(generation);

        peer_connection.on_track(Box::new(move |track, _receiver, _transceiver| {
            let callback_tx = callback_tx_for_track.clone();
            let media_tasks = Arc::clone(&media_tasks);
            Box::pin(async move {
                let codec_name = track.codec().capability.mime_type.clone();
                let track_kind = track.kind();

                info!(
                    event = "track_received",
                    kind = %track_kind,
                    codec_bytes = codec_name.len(),
                    ssrc = track.ssrc(),
                    "Received media track from peer"
                );

                if !supported_codec(track_kind, &track.codec().capability) {
                    warn!(
                        event = "track_codec_unsupported",
                        codec_bytes = codec_name.len(),
                        "Rejected unsupported media codec"
                    );
                    return;
                }
                let mut tasks = media_tasks.lock().unwrap();
                let Some(tasks) = tasks.as_mut() else {
                    return;
                };
                tasks.retain(|task| !task.is_finished());
                match track_kind {
                    RTPCodecType::Video => tasks.push(spawn_video_handler(track, callback_tx)),
                    RTPCodecType::Audio => tasks.push(spawn_audio_handler(track, callback_tx)),
                    _ => warn!(
                        event = "track_kind_unhandled",
                        "Received unknown track kind"
                    ),
                }
            })
        }));

        // Setup data channel handlers
        let ready_state_clone = Arc::clone(&ready_state);
        let incoming_tx_clone = incoming_tx.clone();
        let ingress_generation_clone = Arc::clone(&ingress_generation);
        peer_connection.on_data_channel(Box::new(move |dc: Arc<RTCDataChannel>| {
            let ready_state = Arc::clone(&ready_state_clone);
            let incoming_tx = incoming_tx_clone.clone();
            let current_generation = Arc::clone(&ingress_generation_clone);
            Box::pin(async move {
                bind_data_channel_handlers(
                    dc,
                    ready_state,
                    incoming_tx,
                    generation,
                    current_generation,
                );
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
            generation,
            ingress_generation,
        );

        Ok((peer_connection, data_channel, audio_track))
    }
    .await;

    result
}

fn supported_codec(kind: RTPCodecType, codec: &RTCRtpCodecCapability) -> bool {
    match kind {
        RTPCodecType::Video => {
            codec.mime_type.eq_ignore_ascii_case("video/h264") && codec.clock_rate == 90_000
        }
        RTPCodecType::Audio => {
            codec.mime_type.eq_ignore_ascii_case("audio/opus")
                && codec.clock_rate == 48_000
                && codec.channels == 2
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossbeam_channel::bounded;
    use std::sync::atomic::{AtomicU64, AtomicU8};

    #[tokio::test]
    async fn canceled_partial_setup_closes_owned_peer_and_media() {
        crate::infrastructure::ensure_crypto_provider();
        let peer = Arc::new(
            APIBuilder::new()
                .build()
                .new_peer_connection(RTCConfiguration::default())
                .await
                .unwrap(),
        );
        let resources = PeerResources::new(peer.clone());
        let media = tokio::spawn(std::future::pending::<()>());
        let media_abort = media.abort_handle();
        resources
            .media
            .lock()
            .unwrap()
            .as_mut()
            .unwrap()
            .push(media);
        let (entered, ready) = tokio::sync::oneshot::channel();
        let setup = tokio::spawn(async move {
            let _resources = resources;
            let _ = entered.send(());
            std::future::pending::<()>().await;
        });
        ready.await.unwrap();
        setup.abort();
        let _ = setup.await;
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            while peer.connection_state()
                != webrtc::peer_connection::peer_connection_state::RTCPeerConnectionState::Closed
                || !media_abort.is_finished()
            {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("canceled setup must close its partial peer and abort owned media");
    }

    #[tokio::test]
    async fn advertises_only_implemented_media_codecs() {
        let (incoming_tx, _incoming_rx) = bounded(4);
        let (callback_tx, _callback_rx) = event_channel(4).unwrap();
        let owner = Arc::new(Mutex::new(None));
        let (peer, _, _) = create_peer_connection(
            incoming_tx,
            callback_tx,
            Arc::new(AtomicU8::new(0)),
            0,
            Arc::new(AtomicU64::new(0)),
            Arc::clone(&owner),
        )
        .await
        .unwrap();
        let offer = peer.create_offer(None).await.unwrap();
        let resources = owner.lock().unwrap().take().unwrap();
        tokio::spawn(async move { resources.close().await })
            .await
            .unwrap();
        let codecs: Vec<_> = offer
            .sdp
            .lines()
            .filter(|line| line.starts_with("a=rtpmap:"))
            .collect();
        assert!(codecs.iter().any(|line| line.contains("H264/90000")));
        assert!(codecs.iter().any(|line| line.contains("opus/48000/2")));
        assert!(
            codecs
                .iter()
                .all(|line| line.contains("H264/90000") || line.contains("opus/48000/2")),
            "{codecs:?}"
        );
    }
}

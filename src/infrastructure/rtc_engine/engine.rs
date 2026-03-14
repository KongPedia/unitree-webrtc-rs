use crate::infrastructure::rtc_engine::sdp::parse_answer_payload;
use crate::infrastructure::rtc_engine::setup::create_peer_connection;
use crate::infrastructure::rtc_engine::state::{ReadyStateHolder, RtcReadyState};
use crate::protocol::models::{CallbackEvent, DcMessage};
use crate::protocol::ports::{DataChannelPort, PortResult, RtcEnginePort};
use bytes::Bytes;
use crossbeam_channel::Sender;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::Mutex as StdMutex;
use std::time::{Duration, Instant};
use tokio::sync::mpsc::{unbounded_channel, UnboundedSender};
use tokio::sync::Mutex;
use tracing::info;
use webrtc::data_channel::RTCDataChannel;
use webrtc::peer_connection::RTCPeerConnection;
use webrtc::track::track_local::track_local_static_rtp::TrackLocalStaticRTP;

#[derive(Debug, Clone)]
enum EngineCommand {
    Text(String),
    Binary(Vec<u8>),
}

#[derive(Clone)]
pub struct RtcEngine {
    incoming_tx: Sender<DcMessage>,
    callback_tx: Sender<CallbackEvent>,
    ready_state: ReadyStateHolder,
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
            ready_state: ReadyStateHolder::new(),
            peer_connection: Arc::new(Mutex::new(None)),
            data_channel: Arc::new(Mutex::new(None)),
            command_tx: Arc::new(StdMutex::new(None)),
            audio_track: Arc::new(Mutex::new(None)),
        }
    }

    pub async fn prepare_offer(&self) -> PortResult<String> {
        self.ready_state.set(RtcReadyState::Connecting);

        let (peer_connection, data_channel, audio_track) = create_peer_connection(
            self.incoming_tx.clone(),
            self.callback_tx.clone(),
            self.ready_state.arc(),
        )
        .await?;

        // Store components
        *self.peer_connection.lock().await = Some(Arc::clone(&peer_connection));
        *self.data_channel.lock().await = Some(Arc::clone(&data_channel));
        *self.audio_track.lock().await = Some(audio_track);

        // Setup command channel for sending
        let (command_tx, mut command_rx) = unbounded_channel::<EngineCommand>();
        *self.command_tx.lock().unwrap() = Some(command_tx);

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

        // Create and gather SDP offer
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

        // Wait for data channel to open
        let timeout = Duration::from_secs(30);
        let started_at = Instant::now();
        loop {
            if self.ready_state.get() == RtcReadyState::Open {
                return Ok(());
            }

            if started_at.elapsed() > timeout {
                return Err("Data channel did not open in time".to_string());
            }

            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    pub async fn close(&self) {
        // Close command channel
        *self.command_tx.lock().unwrap() = None;

        // Close data channel
        let data_channel = self.data_channel.lock().await.take();
        if let Some(channel) = data_channel {
            let _ = channel.close().await;
        }

        // Close peer connection
        let peer_connection = self.peer_connection.lock().await.take();
        if let Some(connection) = peer_connection {
            let _ = connection.close().await;
        }

        self.ready_state.set(RtcReadyState::Closed);
    }

    pub fn route_text_message(&self, payload: String) -> PortResult<()> {
        self.incoming_tx
            .try_send(DcMessage::Text(payload))
            .map_err(|error| error.to_string())
    }

    pub fn route_binary_message(&self, payload: Vec<u8>) -> PortResult<()> {
        self.incoming_tx
            .try_send(DcMessage::Binary(payload.into()))
            .map_err(|e| format!("Failed to send binary message to incoming queue: {e}"))?;
        Ok(())
    }

    pub fn current_ready_state(&self) -> RtcReadyState {
        self.ready_state.get()
    }

    pub async fn get_audio_track(&self) -> Option<Arc<TrackLocalStaticRTP>> {
        self.audio_track.lock().await.clone()
    }
}

impl DataChannelPort for RtcEngine {
    fn send_text(&self, message: &str) -> PortResult<()> {
        if self.ready_state.get() != RtcReadyState::Open {
            return Err("DataChannel is not open".to_string());
        }

        let command_tx = self
            .command_tx
            .lock()
            .unwrap()
            .clone()
            .ok_or_else(|| "DataChannel command sender is not initialized".to_string())?;

        command_tx
            .send(EngineCommand::Text(message.to_string()))
            .map_err(|error| error.to_string())
    }

    fn send_binary(&self, bytes: &[u8]) -> PortResult<()> {
        if self.ready_state.get() != RtcReadyState::Open {
            return Err("DataChannel is not open".to_string());
        }

        let command_tx = self
            .command_tx
            .lock()
            .unwrap()
            .clone()
            .ok_or_else(|| "DataChannel command sender is not initialized".to_string())?;

        command_tx
            .send(EngineCommand::Binary(bytes.to_vec()))
            .map_err(|error| error.to_string())
    }

    fn set_message_sender(&self, _sender: Sender<DcMessage>) -> PortResult<()> {
        Err("RtcEngine sender is fixed at construction".to_string())
    }

    fn ready_state(&self) -> &'static str {
        self.ready_state.get().as_str()
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

        assert_eq!(rx.recv().unwrap(), DcMessage::Binary(vec![1, 2, 3].into()));
    }
}

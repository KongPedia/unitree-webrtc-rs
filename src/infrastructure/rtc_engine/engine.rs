use crate::infrastructure::rtc_engine::sdp::parse_answer_payload;
use crate::infrastructure::rtc_engine::setup::{create_peer_connection, PeerResources};
use crate::infrastructure::rtc_engine::state::{ReadyStateHolder, RtcReadyState};
use crate::protocol::events::EventSender;
use crate::protocol::models::{DcMessage, IngressMessage};
use crate::protocol::ports::{DataChannelPort, PortResult, RtcEnginePort};
use bytes::Bytes;
use crossbeam_channel::Sender;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::sync::Mutex as StdMutex;
use std::time::{Duration, Instant};
use tokio::sync::mpsc::{channel, Sender as CommandSender};
use tokio::sync::Mutex;
use tokio::task::JoinHandle;
use tracing::{info, warn};
use webrtc::data_channel::RTCDataChannel;
use webrtc::dtls_transport::dtls_transport_state::RTCDtlsTransportState;
use webrtc::ice_transport::ice_connection_state::RTCIceConnectionState;
use webrtc::peer_connection::peer_connection_state::RTCPeerConnectionState;
use webrtc::peer_connection::RTCPeerConnection;
use webrtc::sctp_transport::sctp_transport_state::RTCSctpTransportState;
use webrtc::track::track_local::track_local_static_rtp::TrackLocalStaticRTP;

// Safety policy, not a robot/protocol maximum: bound queued count and copied bytes.
const COMMAND_QUEUE_CAPACITY: usize = 128;
const MAX_COMMAND_BYTES: usize = 1024 * 1024;

#[derive(Default)]
struct CloseTask {
    task: Mutex<Option<JoinHandle<()>>>,
}

impl CloseTask {
    async fn run(&self, cleanup: impl Future<Output = ()> + Send + 'static) {
        let mut task = self.task.lock().await;
        if task.is_none() {
            *task = Some(tokio::spawn(cleanup));
        }
        // The stored handle owns teardown; canceling a waiter must not cancel close.
        if task.as_mut().expect("close task installed").await.is_err() {
            warn!(
                event = "peer_cleanup_task_failed",
                "Peer cleanup worker failed"
            );
        }
        task.take();
    }
}

struct CommandTask(JoinHandle<()>);

impl Drop for CommandTask {
    fn drop(&mut self) {
        self.0.abort();
    }
}

#[derive(Debug, Clone)]
enum EngineCommand {
    Text(String),
    Binary(Vec<u8>),
}

#[derive(Clone)]
pub struct RtcEngine {
    incoming_tx: Sender<IngressMessage>,
    ingress_generation: Arc<AtomicU64>,
    callback_tx: EventSender,
    ready_state: ReadyStateHolder,
    peer_connection: Arc<Mutex<Option<Arc<RTCPeerConnection>>>>,
    data_channel: Arc<Mutex<Option<Arc<RTCDataChannel>>>>,
    command_tx: Arc<StdMutex<Option<CommandSender<EngineCommand>>>>,
    audio_track: Arc<Mutex<Option<Arc<TrackLocalStaticRTP>>>>,
    command_task: Arc<StdMutex<Option<CommandTask>>>,
    transport_error: Arc<StdMutex<Option<IngressMessage>>>,
    peer_resources: Arc<StdMutex<Option<PeerResources>>>,
    close_task: Arc<CloseTask>,
}

async fn send_commands<F, Fut>(
    mut commands: tokio::sync::mpsc::Receiver<EngineCommand>,
    generation: u64,
    current: &AtomicU64,
    state: &std::sync::atomic::AtomicU8,
    mut send: F,
) -> PortResult<()>
where
    F: FnMut(EngineCommand) -> Fut,
    Fut: Future<Output = PortResult<()>>,
{
    while let Some(command) = commands.recv().await {
        if generation != current.load(Ordering::SeqCst) {
            break;
        }
        if RtcReadyState::from_u8(state.load(Ordering::SeqCst)) != RtcReadyState::Open {
            return Err("DataChannel closed before queued command was sent".into());
        }
        send(command).await?;
    }
    Ok(())
}

fn record_transport_fault(
    fault: &StdMutex<Option<IngressMessage>>,
    incoming: &Sender<IngressMessage>,
    generation: u64,
    current: &AtomicU64,
    state: &std::sync::atomic::AtomicU8,
) {
    let mut fault = fault.lock().unwrap();
    if generation != current.load(Ordering::SeqCst) {
        return;
    }
    state.store(RtcReadyState::Closed as u8, Ordering::SeqCst);
    let message =
        DcMessage::TransportFault("DataChannel send failed".into()).for_generation(generation);
    *fault = Some(message.clone());
    // The retained fault is authoritative; a full telemetry queue cannot drop it.
    let _ = incoming.try_send(message);
}

fn channel_open_timeout_error(
    peer: RTCPeerConnectionState,
    ice: RTCIceConnectionState,
    dtls: RTCDtlsTransportState,
    sctp: RTCSctpTransportState,
    channel: RtcReadyState,
) -> String {
    format!("Data channel did not open in time: peer={peer}, ice={ice}, dtls={dtls}, sctp={sctp}, channel={}", channel.as_str())
}

impl RtcEngine {
    pub fn new(incoming_tx: Sender<IngressMessage>, callback_tx: EventSender) -> Self {
        crate::infrastructure::ensure_crypto_provider();
        Self {
            incoming_tx,
            ingress_generation: Arc::new(AtomicU64::new(0)),
            callback_tx,
            ready_state: ReadyStateHolder::new(),
            peer_connection: Arc::new(Mutex::new(None)),
            data_channel: Arc::new(Mutex::new(None)),
            command_tx: Arc::new(StdMutex::new(None)),
            audio_track: Arc::new(Mutex::new(None)),
            command_task: Arc::new(StdMutex::new(None)),
            transport_error: Arc::new(StdMutex::new(None)),
            peer_resources: Arc::new(StdMutex::new(None)),
            close_task: Arc::new(CloseTask::default()),
        }
    }

    pub async fn prepare_offer(&self) -> PortResult<String> {
        self.close().await;
        self.ready_state.reset();
        let result = self.prepare_offer_inner().await;
        if result.is_err() {
            self.close().await;
        }
        result
    }

    async fn prepare_offer_inner(&self) -> PortResult<String> {
        self.ready_state.set(RtcReadyState::Connecting);
        let generation = self.ingress_generation.load(Ordering::SeqCst);

        let (peer_connection, data_channel, audio_track) = create_peer_connection(
            self.incoming_tx.clone(),
            self.callback_tx.clone(),
            self.ready_state.arc(),
            generation,
            Arc::clone(&self.ingress_generation),
            Arc::clone(&self.peer_resources),
        )
        .await?;

        // Store components
        *self.peer_connection.lock().await = Some(Arc::clone(&peer_connection));
        *self.data_channel.lock().await = Some(Arc::clone(&data_channel));
        *self.audio_track.lock().await = Some(audio_track);

        // Setup command channel for sending
        let (command_tx, command_rx) = channel::<EngineCommand>(COMMAND_QUEUE_CAPACITY);
        *self.command_tx.lock().unwrap() = Some(command_tx);

        let data_channel_for_send = Arc::clone(&data_channel);
        let send_state = self.ready_state.arc();
        let fault = Arc::clone(&self.transport_error);
        let incoming = self.incoming_tx.clone();
        let current_generation = Arc::clone(&self.ingress_generation);
        let task = tokio::spawn(async move {
            let result = send_commands(
                command_rx,
                generation,
                &current_generation,
                &send_state,
                move |command| {
                    let data_channel = Arc::clone(&data_channel_for_send);
                    async move {
                        match command {
                            EngineCommand::Text(text) => data_channel.send_text(text).await,
                            EngineCommand::Binary(binary) => {
                                data_channel.send(&Bytes::from(binary)).await
                            }
                        }
                        .map(|_| ())
                        .map_err(|error| error.to_string())
                    }
                },
            )
            .await;
            if result.is_err() {
                warn!(
                    event = "dc_send_failed",
                    "Queued datachannel command failed"
                );
                record_transport_fault(
                    &fault,
                    &incoming,
                    generation,
                    &current_generation,
                    &send_state,
                );
            }
        });
        if let Some(resources) = self.peer_resources.lock().unwrap().as_mut() {
            resources.own_command(task.abort_handle());
        } else {
            task.abort();
            return Err("Peer closed while preparing offer".into());
        }
        *self.command_task.lock().unwrap() = Some(CommandTask(task));

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
            match self.ready_state.get() {
                RtcReadyState::Open => return Ok(()),
                RtcReadyState::Closed | RtcReadyState::Failed => {
                    return Err("Data channel closed or failed before opening".to_string())
                }
                RtcReadyState::Connecting | RtcReadyState::Disconnected => {}
            }

            if started_at.elapsed() > timeout {
                let diagnostic = channel_open_timeout_error(
                    peer_connection.connection_state(),
                    peer_connection.ice_connection_state(),
                    peer_connection.dtls_transport().state(),
                    peer_connection.sctp().state(),
                    self.ready_state.get(),
                );
                warn!(event = "datachannel_open_timeout", detail = %diagnostic, "Transport did not open before deadline");
                return Err(diagnostic);
            }

            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    pub async fn close(&self) {
        let engine = self.clone();
        self.close_task
            .run(async move { engine.close_inner().await })
            .await;
    }

    async fn close_inner(&self) {
        self.ready_state.set(RtcReadyState::Closed);
        *self.command_tx.lock().unwrap() = None;
        let task = self.command_task.lock().unwrap().take();
        if let Some(mut task) = task {
            task.0.abort();
            let _ = (&mut task.0).await;
        }
        *self.audio_track.lock().await = None;

        let resources = self.peer_resources.lock().unwrap().take();
        let peer_owned = resources.is_some();
        if let Some(resources) = resources {
            resources.close().await;
        }

        // Close data channel
        let data_channel = self.data_channel.lock().await.take();
        if let Some(channel) = data_channel.filter(|_| !peer_owned) {
            if channel.close().await.is_err() {
                warn!(event = "dc_close_failed", "Failed to close datachannel");
            }
        }

        // Close peer connection
        let peer_connection = self.peer_connection.lock().await.take();
        if let Some(connection) = peer_connection.filter(|_| !peer_owned) {
            if connection.close().await.is_err() {
                warn!(
                    event = "peer_close_failed",
                    "Failed to close peer connection"
                );
            }
        }

        self.ready_state.set(RtcReadyState::Closed);
    }

    pub fn route_text_message(&self, payload: String) -> PortResult<()> {
        self.incoming_tx
            .try_send(
                DcMessage::Text(payload)
                    .for_generation(self.ingress_generation.load(Ordering::SeqCst)),
            )
            .map_err(|error| error.to_string())
    }

    pub fn route_binary_message(&self, payload: Vec<u8>) -> PortResult<()> {
        self.incoming_tx
            .try_send(
                DcMessage::Binary(payload.into())
                    .for_generation(self.ingress_generation.load(Ordering::SeqCst)),
            )
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
        if message.len() > MAX_COMMAND_BYTES {
            return Err("DataChannel command exceeds byte limit".to_string());
        }
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
            .try_send(EngineCommand::Text(message.to_string()))
            .map_err(|error| error.to_string())
    }

    fn send_binary(&self, bytes: &[u8]) -> PortResult<()> {
        if bytes.len() > MAX_COMMAND_BYTES {
            return Err("DataChannel command exceeds byte limit".to_string());
        }
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
            .try_send(EngineCommand::Binary(bytes.to_vec()))
            .map_err(|error| error.to_string())
    }

    fn set_message_sender(&self, _sender: Sender<IngressMessage>) -> PortResult<()> {
        Err("RtcEngine sender is fixed at construction".to_string())
    }

    fn set_ingress_generation(&self, generation: u64) -> PortResult<()> {
        let mut fault = self.transport_error.lock().unwrap();
        *fault = None;
        if self.ingress_generation.swap(generation, Ordering::SeqCst) != generation {
            self.ready_state.set(RtcReadyState::Closed);
        }
        Ok(())
    }

    fn take_transport_error(&self) -> Option<IngressMessage> {
        self.transport_error.lock().unwrap().take()
    }

    fn ready_state(&self) -> &'static str {
        self.ready_state.get().as_str()
    }
}

impl RtcEnginePort for RtcEngine {
    fn timeout_diagnostic(&self) -> Option<String> {
        // Never extend the outer deadline while waiting for either owner lock.
        let peer = self.peer_connection.try_lock().ok()?;
        let peer = peer.as_ref()?;
        Some(channel_open_timeout_error(
            peer.connection_state(),
            peer.ice_connection_state(),
            peer.dtls_transport().state(),
            peer.sctp().state(),
            self.ready_state.try_get()?,
        ))
    }

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
    use crate::protocol::events::event_channel;
    use crossbeam_channel::bounded;

    #[tokio::test]
    async fn cancelled_early_setup_retains_tracked_cleanup_before_next_attempt() {
        let (incoming, _rx) = bounded(4);
        let (callbacks, _events) = event_channel(4).unwrap();
        let engine = RtcEngine::new(incoming, callbacks);
        let peer = Arc::new(
            webrtc::api::APIBuilder::new()
                .build()
                .new_peer_connection(
                    webrtc::peer_connection::configuration::RTCConfiguration::default(),
                )
                .await
                .unwrap(),
        );
        let (registered, registered_rx) = tokio::sync::oneshot::channel();
        let (closing, closing_rx) = tokio::sync::oneshot::channel();
        let (resume, resume_rx) = tokio::sync::oneshot::channel();
        let setup = {
            let owner = engine.peer_resources.clone();
            let peer = peer.clone();
            tokio::spawn(async move {
                let _media = PeerResources::register(peer, &owner);
                owner.lock().unwrap().as_mut().unwrap().cleanup_barrier =
                    Some((closing, resume_rx));
                registered.send(()).unwrap();
                std::future::pending::<()>().await;
            })
        };
        registered_rx.await.unwrap();
        setup.abort();
        setup.await.unwrap_err();
        assert!(engine.peer_resources.lock().unwrap().is_some());
        let mut close = {
            let engine = engine.clone();
            tokio::spawn(async move { engine.close().await })
        };
        closing_rx.await.unwrap();
        let mut prepare = {
            let engine = engine.clone();
            tokio::spawn(async move { engine.prepare_offer().await })
        };
        assert!(tokio::time::timeout(Duration::from_millis(50), &mut close)
            .await
            .is_err());
        assert!(
            tokio::time::timeout(Duration::from_millis(50), &mut prepare)
                .await
                .is_err()
        );
        prepare.abort();
        prepare.await.unwrap_err();
        resume.send(()).unwrap();
        close.await.unwrap();
        assert_eq!(peer.connection_state(), RTCPeerConnectionState::Closed);
        assert!(engine.peer_resources.lock().unwrap().is_none());
    }

    #[tokio::test]
    async fn cancelled_close_waiter_preserves_cleanup_and_blocks_next_attempt() {
        let (incoming, _rx) = bounded(4);
        let (callbacks, _events) = event_channel(4).unwrap();
        let engine = RtcEngine::new(incoming, callbacks);
        let cleanup = engine.close_task.clone();
        let (started, started_rx) = tokio::sync::oneshot::channel();
        let (resume, resume_rx) = tokio::sync::oneshot::channel();
        let completed = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let waiter = {
            let cleanup = cleanup.clone();
            let completed = completed.clone();
            tokio::spawn(async move {
                cleanup
                    .run(async move {
                        started.send(()).unwrap();
                        resume_rx.await.unwrap();
                        completed.store(true, Ordering::SeqCst);
                    })
                    .await;
            })
        };
        started_rx.await.unwrap();
        waiter.abort();
        waiter.await.unwrap_err();
        let mut next = {
            let engine = engine.clone();
            tokio::spawn(async move { engine.close().await })
        };
        let mut prepare = {
            let engine = engine.clone();
            tokio::spawn(async move { engine.prepare_offer().await })
        };
        assert!(
            tokio::time::timeout(Duration::from_millis(50), &mut next)
                .await
                .is_err(),
            "next attempt returned before the original cleanup completed"
        );
        assert!(
            tokio::time::timeout(Duration::from_millis(50), &mut prepare)
                .await
                .is_err()
        );
        assert_eq!(engine.current_ready_state(), RtcReadyState::Closed);
        prepare.abort();
        prepare.await.unwrap_err();
        resume
            .send(())
            .expect("cleanup worker must outlive its canceled waiter");
        next.await.unwrap();
        assert!(completed.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn queued_commands_are_not_sent_after_generation_reset_or_transport_close() {
        for reset_generation in [true, false] {
            let (incoming, _rx) = bounded(4);
            let (callbacks, _events) = event_channel(4).unwrap();
            let engine = RtcEngine::new(incoming, callbacks);
            engine.set_ingress_generation(7).unwrap();
            engine.ready_state.set(RtcReadyState::Open);
            let (tx, rx) = channel(4);
            tx.send(EngineCommand::Text("first".into())).await.unwrap();
            tx.send(EngineCommand::Text("second".into())).await.unwrap();
            drop(tx);
            let (started_tx, started_rx) = tokio::sync::oneshot::channel();
            let (resume_tx, resume_rx) = tokio::sync::oneshot::channel();
            let sent = Arc::new(StdMutex::new(Vec::new()));
            let sent_worker = sent.clone();
            let current = engine.ingress_generation.clone();
            let state = engine.ready_state.arc();
            let worker = tokio::spawn(async move {
                let mut barrier = Some((started_tx, resume_rx));
                send_commands(rx, 7, &current, &state, move |command| {
                    sent_worker.lock().unwrap().push(command);
                    let barrier = barrier.take();
                    async move {
                        if let Some((started, resume)) = barrier {
                            started.send(()).unwrap();
                            resume.await.unwrap();
                        }
                        Ok(())
                    }
                })
                .await
            });
            started_rx.await.unwrap();
            if reset_generation {
                engine.set_ingress_generation(8).unwrap();
                engine.ready_state.set(RtcReadyState::Open);
            } else {
                engine.ready_state.set(RtcReadyState::Closed);
            }
            resume_tx.send(()).unwrap();
            assert_eq!(worker.await.unwrap().is_ok(), reset_generation);
            assert_eq!(
                sent.lock().unwrap().len(),
                1,
                "queued old command reached send after invalidation"
            );
        }
    }

    #[tokio::test]
    async fn disconnected_dequeued_command_fails_epoch_before_late_peer_recovery() {
        use crate::infrastructure::rtc_engine::state::{apply_event, RtcStateEvent};

        let (incoming, _rx) = bounded(1);
        let (callbacks, _events) = event_channel(1).unwrap();
        let engine = RtcEngine::new(incoming.clone(), callbacks);
        engine.set_ingress_generation(7).unwrap();
        engine.ready_state.set(RtcReadyState::Open);
        let state = engine.ready_state.arc();
        let (tx, rx) = channel(4);
        *engine.command_tx.lock().unwrap() = Some(tx);
        engine.send_text("accepted before disconnect").unwrap();
        apply_event(&state, RtcStateEvent::PeerDisconnected);
        let result = send_commands(rx, 7, &engine.ingress_generation, &state, |_| async {
            panic!("disconnected command must not reach the wire");
        })
        .await;
        assert!(
            result.is_err(),
            "losing the command receiver must fail the epoch"
        );
        record_transport_fault(
            &engine.transport_error,
            &incoming,
            7,
            &engine.ingress_generation,
            &state,
        );
        apply_event(&state, RtcStateEvent::PeerConnected);
        assert_eq!(engine.current_ready_state(), RtcReadyState::Closed);
        assert!(engine.send_text("late text").is_err());
        assert!(engine.send_binary(&[1]).is_err());
        assert!(matches!(
            engine.take_transport_error(),
            Some(IngressMessage {
                generation: 7,
                payload: DcMessage::TransportFault(_)
            })
        ));
    }

    #[test]
    fn terminal_send_fault_survives_full_ingress_and_rejects_old_generation() {
        let (tx, _rx) = bounded(1);
        tx.send(DcMessage::Text("queued telemetry".into()).for_generation(9))
            .unwrap();
        let (callback_tx, _callback_rx) = event_channel(1).unwrap();
        let engine = RtcEngine::new(tx.clone(), callback_tx);
        engine.set_ingress_generation(9).unwrap();
        engine.ready_state.set(RtcReadyState::Open);
        record_transport_fault(
            &engine.transport_error,
            &tx,
            9,
            &engine.ingress_generation,
            &engine.ready_state.arc(),
        );
        assert_eq!(
            engine.take_transport_error(),
            Some(DcMessage::TransportFault("DataChannel send failed".into()).for_generation(9))
        );
        engine.set_ingress_generation(10).unwrap();
        engine.ready_state.set(RtcReadyState::Open);
        record_transport_fault(
            &engine.transport_error,
            &tx,
            9,
            &engine.ingress_generation,
            &engine.ready_state.arc(),
        );
        assert!(engine.take_transport_error().is_none());
        assert_eq!(engine.current_ready_state(), RtcReadyState::Open);
    }

    #[tokio::test]
    async fn outer_deadline_snapshot_reads_peer_states_without_waiting_for_owner() {
        let (incoming, _rx) = bounded(1);
        let (callbacks, _events) = event_channel(1).unwrap();
        let engine = RtcEngine::new(incoming, callbacks);
        assert!(engine.timeout_diagnostic().is_none());
        let peer = Arc::new(
            webrtc::api::APIBuilder::new()
                .build()
                .new_peer_connection(
                    webrtc::peer_connection::configuration::RTCConfiguration::default(),
                )
                .await
                .unwrap(),
        );
        let expected = channel_open_timeout_error(
            peer.connection_state(),
            peer.ice_connection_state(),
            peer.dtls_transport().state(),
            peer.sctp().state(),
            RtcReadyState::Connecting,
        );
        engine.ready_state.set(RtcReadyState::Connecting);
        *engine.peer_connection.lock().await = Some(peer);
        assert_eq!(engine.timeout_diagnostic(), Some(expected));
        {
            let _owner = engine.peer_connection.lock().await;
            assert!(engine.timeout_diagnostic().is_none());
        }
        engine.close().await;
    }

    #[test]
    fn channel_timeout_snapshot_distinguishes_transport_states_without_peer_payload() {
        let ice_stall = channel_open_timeout_error(
            RTCPeerConnectionState::Connecting,
            RTCIceConnectionState::Checking,
            RTCDtlsTransportState::New,
            RTCSctpTransportState::Connecting,
            RtcReadyState::Connecting,
        );
        assert_eq!(ice_stall, "Data channel did not open in time: peer=connecting, ice=checking, dtls=new, sctp=connecting, channel=connecting");
        let dtls_stall = channel_open_timeout_error(
            RTCPeerConnectionState::Connecting,
            RTCIceConnectionState::Connected,
            RTCDtlsTransportState::Connecting,
            RTCSctpTransportState::Connecting,
            RtcReadyState::Disconnected,
        );
        assert_eq!(dtls_stall, "Data channel did not open in time: peer=connecting, ice=connected, dtls=connecting, sctp=connecting, channel=disconnected");
        assert_ne!(ice_stall, dtls_stall);
        assert!(ice_stall.len() < 256 && dtls_stall.len() < 256);
    }

    #[test]
    fn routes_text_message_to_channel() {
        let (tx, rx) = bounded::<IngressMessage>(4);
        let (callback_tx, _callback_rx) = event_channel(4).unwrap();
        let engine = RtcEngine::new(tx, callback_tx);

        engine.route_text_message("hello".to_string()).unwrap();

        assert_eq!(
            rx.recv().unwrap().payload,
            DcMessage::Text("hello".to_string())
        );
    }

    #[test]
    fn routes_binary_message_to_channel() {
        let (tx, rx) = bounded::<IngressMessage>(4);
        let (callback_tx, _callback_rx) = event_channel(4).unwrap();
        let engine = RtcEngine::new(tx, callback_tx);

        engine.route_binary_message(vec![1, 2, 3]).unwrap();

        assert_eq!(
            rx.recv().unwrap().payload,
            DcMessage::Binary(vec![1, 2, 3].into())
        );
    }
    #[test]
    fn rejects_oversized_command_before_enqueue() {
        let (tx, _rx) = bounded::<IngressMessage>(4);
        let (callback_tx, _callback_rx) = event_channel(4).unwrap();
        let engine = RtcEngine::new(tx, callback_tx);
        let (command_tx, mut command_rx) = channel(COMMAND_QUEUE_CAPACITY);
        *engine.command_tx.lock().unwrap() = Some(command_tx);
        engine.ready_state.set(RtcReadyState::Open);
        assert!(engine.send_binary(&vec![0; 1024 * 1024 + 1]).is_err());
        assert!(command_rx.try_recv().is_err());
    }
    #[test]
    fn full_command_queue_returns_error_without_blocking() {
        let (tx, _rx) = bounded::<IngressMessage>(4);
        let (callback_tx, _callback_rx) = event_channel(4).unwrap();
        let engine = RtcEngine::new(tx, callback_tx);
        let (command_tx, mut command_rx) = channel(1);
        *engine.command_tx.lock().unwrap() = Some(command_tx);
        engine.ready_state.set(RtcReadyState::Open);
        engine.send_text("first").unwrap();
        assert!(engine.send_text("second").is_err());
        command_rx.try_recv().unwrap();
        engine.send_binary(&[1]).unwrap();
    }

    #[tokio::test]
    async fn close_cancels_and_joins_command_worker() {
        let (tx, _rx) = bounded::<IngressMessage>(4);
        let (callback_tx, _callback_rx) = event_channel(4).unwrap();
        let engine = RtcEngine::new(tx, callback_tx);
        let worker = tokio::spawn(std::future::pending::<()>());
        let aborted = worker.abort_handle();
        *engine.command_task.lock().unwrap() = Some(CommandTask(worker));
        engine.close().await;
        assert!(aborted.is_finished());
        assert!(engine.command_task.lock().unwrap().is_none());
        assert!(engine.command_tx.lock().unwrap().is_none());
        assert!(engine.get_audio_track().await.is_none());
        assert_eq!(engine.current_ready_state(), RtcReadyState::Closed);
    }
}

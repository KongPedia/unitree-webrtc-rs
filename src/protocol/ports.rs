use super::models::IngressMessage;
use crossbeam_channel::Sender;
use std::future::Future;
use std::pin::Pin;

pub type PortResult<T> = Result<T, String>;

pub trait SignalingPort: Send + Sync {
    fn exchange_sdp<'a>(
        &'a self,
        ip: &'a str,
        offer: &'a str,
    ) -> Pin<Box<dyn Future<Output = PortResult<String>> + Send + 'a>>;
}

pub trait DataChannelPort: Send + Sync {
    fn send_text(&self, message: &str) -> PortResult<()>;
    fn send_binary(&self, bytes: &[u8]) -> PortResult<()>;
    fn set_message_sender(&self, sender: Sender<IngressMessage>) -> PortResult<()>;
    fn set_ingress_generation(&self, generation: u64) -> PortResult<()>;
    /// A generation-tagged fault retained even when the ingress queue is full.
    fn take_transport_error(&self) -> Option<IngressMessage> {
        None
    }
    fn ready_state(&self) -> &'static str;
}

pub trait RtcEnginePort: Send + Sync {
    /// Nonblocking deadline snapshot containing only fixed transport-state labels.
    /// Never include SDP, peer addresses, credentials or arbitrary peer/user text.
    fn timeout_diagnostic(&self) -> Option<String> {
        None
    }
    fn prepare_offer<'a>(&'a self)
        -> Pin<Box<dyn Future<Output = PortResult<String>> + Send + 'a>>;
    fn apply_answer<'a>(
        &'a self,
        answer_sdp: &'a str,
    ) -> Pin<Box<dyn Future<Output = PortResult<()>> + Send + 'a>>;
    fn close<'a>(&'a self) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>>;
}

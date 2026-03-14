use super::models::DcMessage;
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
    fn set_message_sender(&self, sender: Sender<DcMessage>) -> PortResult<()>;
    fn ready_state(&self) -> &'static str;
}

pub trait RtcEnginePort: Send + Sync {
    fn prepare_offer<'a>(&'a self)
        -> Pin<Box<dyn Future<Output = PortResult<String>> + Send + 'a>>;
    fn apply_answer<'a>(
        &'a self,
        answer_sdp: &'a str,
    ) -> Pin<Box<dyn Future<Output = PortResult<()>> + Send + 'a>>;
    fn close<'a>(&'a self) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>>;
}

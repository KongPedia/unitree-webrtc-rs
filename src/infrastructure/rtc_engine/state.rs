use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Arc;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum RtcReadyState {
    Closed = 0,
    Connecting = 1,
    Open = 2,
}

impl RtcReadyState {
    pub fn from_u8(value: u8) -> Self {
        match value {
            1 => Self::Connecting,
            2 => Self::Open,
            _ => Self::Closed,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Closed => "closed",
            Self::Connecting => "connecting",
            Self::Open => "open",
        }
    }
}

pub struct ReadyStateHolder {
    inner: Arc<AtomicU8>,
}

impl ReadyStateHolder {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(AtomicU8::new(RtcReadyState::Closed as u8)),
        }
    }

    pub fn get(&self) -> RtcReadyState {
        RtcReadyState::from_u8(self.inner.load(Ordering::SeqCst))
    }

    pub fn set(&self, state: RtcReadyState) {
        self.inner.store(state as u8, Ordering::SeqCst);
    }

    pub fn arc(&self) -> Arc<AtomicU8> {
        Arc::clone(&self.inner)
    }
}

impl Clone for ReadyStateHolder {
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
        }
    }
}

impl Default for ReadyStateHolder {
    fn default() -> Self {
        Self::new()
    }
}

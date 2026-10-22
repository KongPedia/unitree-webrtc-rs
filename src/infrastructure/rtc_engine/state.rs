use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum RtcReadyState {
    Closed = 0,
    Connecting = 1,
    Open = 2,
    Disconnected = 3,
    Failed = 4,
}

impl RtcReadyState {
    pub fn from_u8(value: u8) -> Self {
        match value {
            1 => Self::Connecting,
            2 => Self::Open,
            3 => Self::Disconnected,
            4 => Self::Failed,
            _ => Self::Closed,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Closed => "closed",
            Self::Connecting => "connecting",
            Self::Open => "open",
            Self::Disconnected => "disconnected",
            Self::Failed => "failed",
        }
    }
}

#[derive(Clone, Copy)]
pub(super) enum RtcStateEvent {
    ChannelOpened,
    ChannelClosed,
    PeerConnected,
    PeerDisconnected,
    PeerFailed,
    PeerClosed,
}

pub(super) fn apply_event(state: &AtomicU8, event: RtcStateEvent) {
    let _ = state.fetch_update(Ordering::SeqCst, Ordering::SeqCst, |value| {
        let current = RtcReadyState::from_u8(value);
        if matches!(current, RtcReadyState::Closed | RtcReadyState::Failed) {
            return Some(value);
        }
        let next = match event {
            RtcStateEvent::ChannelClosed | RtcStateEvent::PeerClosed => RtcReadyState::Closed,
            RtcStateEvent::PeerFailed => RtcReadyState::Failed,
            RtcStateEvent::ChannelOpened if current == RtcReadyState::Connecting => {
                RtcReadyState::Open
            }
            RtcStateEvent::PeerDisconnected if current == RtcReadyState::Open => {
                RtcReadyState::Disconnected
            }
            RtcStateEvent::PeerConnected if current == RtcReadyState::Disconnected => {
                RtcReadyState::Open
            }
            _ => current,
        };
        Some(next as u8)
    });
}

pub struct ReadyStateHolder {
    inner: Arc<Mutex<Arc<AtomicU8>>>,
}

impl ReadyStateHolder {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(Arc::new(AtomicU8::new(
                RtcReadyState::Closed as u8,
            )))),
        }
    }

    pub fn get(&self) -> RtcReadyState {
        RtcReadyState::from_u8(self.arc().load(Ordering::SeqCst))
    }

    pub fn try_get(&self) -> Option<RtcReadyState> {
        let state = self.inner.try_lock().ok()?;
        Some(RtcReadyState::from_u8(state.load(Ordering::SeqCst)))
    }

    pub fn set(&self, state: RtcReadyState) {
        self.arc().store(state as u8, Ordering::SeqCst);
    }

    pub fn arc(&self) -> Arc<AtomicU8> {
        Arc::clone(&self.inner.lock().unwrap())
    }

    pub fn reset(&self) {
        // Each peer captures its own state; old callbacks cannot mutate a new session.
        *self.inner.lock().unwrap() = Arc::new(AtomicU8::new(RtcReadyState::Closed as u8));
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deadline_state_read_does_not_wait_for_state_owner() {
        let state = ReadyStateHolder::new();
        state.set(RtcReadyState::Connecting);
        assert_eq!(state.try_get(), Some(RtcReadyState::Connecting));
        let owner = state.inner.lock().unwrap();
        let reader = state.clone();
        let (sent, received) = std::sync::mpsc::channel();
        let task = std::thread::spawn(move || sent.send(reader.try_get()).unwrap());
        let result = received.recv_timeout(std::time::Duration::from_secs(1));
        drop(owner);
        task.join().unwrap();
        assert_eq!(
            result.expect("deadline read waited for the state-owner lock"),
            None
        );
    }

    #[test]
    fn transient_disconnect_can_recover_after_channel_was_open() {
        let state = AtomicU8::new(RtcReadyState::Open as u8);
        apply_event(&state, RtcStateEvent::PeerDisconnected);
        assert_eq!(
            RtcReadyState::from_u8(state.load(Ordering::SeqCst)),
            RtcReadyState::Disconnected
        );
        apply_event(&state, RtcStateEvent::PeerConnected);
        assert_eq!(
            RtcReadyState::from_u8(state.load(Ordering::SeqCst)),
            RtcReadyState::Open
        );
        let waiting = AtomicU8::new(RtcReadyState::Connecting as u8);
        apply_event(&waiting, RtcStateEvent::PeerDisconnected);
        apply_event(&waiting, RtcStateEvent::PeerConnected);
        assert_eq!(
            RtcReadyState::from_u8(waiting.load(Ordering::SeqCst)),
            RtcReadyState::Connecting,
            "peerconnected alone cannot imply channelopen"
        );
        apply_event(&waiting, RtcStateEvent::ChannelOpened);
        assert_eq!(
            RtcReadyState::from_u8(waiting.load(Ordering::SeqCst)),
            RtcReadyState::Open
        );
    }

    #[test]
    fn terminal_peer_cannot_resurrect_from_late_callbacks() {
        for terminal in [
            RtcStateEvent::PeerFailed,
            RtcStateEvent::PeerClosed,
            RtcStateEvent::ChannelClosed,
        ] {
            let state = AtomicU8::new(RtcReadyState::Open as u8);
            apply_event(&state, terminal);
            let stopped = state.load(Ordering::SeqCst);
            apply_event(&state, RtcStateEvent::PeerDisconnected);
            apply_event(&state, RtcStateEvent::PeerConnected);
            apply_event(&state, RtcStateEvent::ChannelOpened);
            assert_eq!(state.load(Ordering::SeqCst), stopped);
        }
    }

    #[test]
    fn reset_isolates_old_callbacks_and_is_shared_by_clones() {
        let state = ReadyStateHolder::new();
        let clone = state.clone();
        let old_callback = state.arc();
        state.reset();
        state.set(RtcReadyState::Connecting);
        old_callback.store(RtcReadyState::Open as u8, Ordering::SeqCst);
        assert_eq!(clone.get(), RtcReadyState::Connecting);
    }
}

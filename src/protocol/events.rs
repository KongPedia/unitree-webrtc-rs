//! Generation-bound callback admission. Budgets cover queued and dispatch-owned
//! buffers, not whole-process RSS or arrays retained by callback consumers.
use super::models::CallbackEvent;
use super::ports::PortResult;
use crossbeam_channel::{
    bounded, Receiver, RecvError, RecvTimeoutError, Sender, TryRecvError, TrySendError,
};
use serde_json::Value;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const MEDIA_BYTES: usize = 32 * 1024 * 1024;
const CONTROL_BYTES: usize = 2 * 1024 * 1024;

#[derive(Default)]
struct Usage {
    closed: bool,
    bytes: [usize; 2],
    count: [usize; 2],
}
struct Budget {
    usage: Mutex<Usage>,
    capacity: usize,
}
struct Permit {
    budget: Arc<Budget>,
    class: usize,
    bytes: usize,
}
impl Drop for Permit {
    fn drop(&mut self) {
        let mut usage = self.budget.usage.lock().unwrap_or_else(|e| e.into_inner());
        usage.bytes[self.class] -= self.bytes;
        usage.count[self.class] -= 1;
    }
}

/// A captured producer epoch; cloning never retags work as a newer session.
#[derive(Clone)]
pub struct EventSender {
    control: Sender<EventDelivery>,
    media: Sender<EventDelivery>,
    generation: u64,
    current: Arc<AtomicU64>,
    budget: Arc<Budget>,
}

pub struct EventReceiver {
    budget: Arc<Budget>,
    control: Receiver<EventDelivery>,
    media: Receiver<EventDelivery>,
}

pub(crate) struct EventDelivery {
    event: Option<CallbackEvent>,
    generation: u64,
    current: Arc<AtomicU64>,
    _permit: Permit,
}
impl EventDelivery {
    #[cfg(feature = "python")]
    pub(crate) fn generation(&self) -> u64 {
        self.generation
    }
    pub(crate) fn is_current(&self) -> bool {
        self.current.load(Ordering::Acquire) == self.generation
    }
    /// Keep this envelope alive until callback completion to retain admission bytes.
    pub(crate) fn take_event(&mut self) -> CallbackEvent {
        self.event.take().expect("event extracted only once")
    }
}

pub fn event_channel(capacity: usize) -> PortResult<(EventSender, EventReceiver)> {
    if !(1..=256).contains(&capacity) {
        return Err("event capacity must be between 1 and 256".into());
    }
    let (control_tx, control) = bounded(capacity);
    let (media_tx, media) = bounded(capacity.min(4));
    let budget = Arc::new(Budget {
        usage: Mutex::new(Usage::default()),
        capacity,
    });
    Ok((
        EventSender {
            control: control_tx,
            media: media_tx,
            generation: 0,
            current: Arc::new(AtomicU64::new(0)),
            budget: Arc::clone(&budget),
        },
        EventReceiver {
            control,
            media,
            budget,
        },
    ))
}

impl EventSender {
    pub fn for_generation(&self, generation: u64) -> Self {
        Self {
            generation,
            ..self.clone()
        }
    }
    pub fn set_generation(&self, generation: u64) {
        self.current.store(generation, Ordering::Release);
    }
    pub fn generation_state(&self) -> Arc<AtomicU64> {
        Arc::clone(&self.current)
    }

    pub fn try_send(&self, event: CallbackEvent) -> Result<(), TrySendError<CallbackEvent>> {
        if self.current.load(Ordering::Acquire) != self.generation {
            return Err(TrySendError::Full(event));
        }
        let (class, bytes) = event_size(&event);
        let byte_limit = if class == 0 {
            CONTROL_BYTES
        } else {
            MEDIA_BYTES
        };
        let count_limit = if class == 0 {
            self.budget.capacity
        } else {
            self.budget.capacity.min(4)
        };
        let mut usage = self.budget.usage.lock().unwrap_or_else(|e| e.into_inner());
        if usage.closed {
            return Err(TrySendError::Disconnected(event));
        }
        if self.current.load(Ordering::Acquire) != self.generation {
            return Err(TrySendError::Full(event));
        }
        if bytes > byte_limit.saturating_sub(usage.bytes[class])
            || usage.count[class] >= count_limit
        {
            return Err(TrySendError::Full(event));
        }
        usage.bytes[class] += bytes;
        usage.count[class] += 1;
        let delivery = EventDelivery {
            event: Some(event),
            generation: self.generation,
            current: Arc::clone(&self.current),
            _permit: Permit {
                budget: Arc::clone(&self.budget),
                class,
                bytes,
            },
        };
        let sender = if class == 0 {
            &self.control
        } else {
            &self.media
        };
        let result = sender.try_send(delivery);
        drop(usage);
        result.map_err(|error| match error {
            TrySendError::Full(mut delivery) => TrySendError::Full(delivery.take_event()),
            TrySendError::Disconnected(mut delivery) => {
                TrySendError::Disconnected(delivery.take_event())
            }
        })
    }
}

impl Drop for EventReceiver {
    fn drop(&mut self) {
        // Crossbeam retains queued values while producers live; explicitly drain.
        self.budget
            .usage
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .closed = true;
        while self.control.try_recv().is_ok() {}
        while self.media.try_recv().is_ok() {}
    }
}

impl EventReceiver {
    fn try_delivery(&self) -> Result<EventDelivery, TryRecvError> {
        loop {
            let delivery = match self.control.try_recv() {
                Ok(event) => event,
                Err(control_error) => match self.media.try_recv() {
                    Ok(event) => event,
                    Err(TryRecvError::Disconnected)
                        if control_error == TryRecvError::Disconnected =>
                    {
                        return Err(TryRecvError::Disconnected)
                    }
                    Err(_) => return Err(TryRecvError::Empty),
                },
            };
            if delivery.is_current() {
                return Ok(delivery);
            }
        }
    }
    pub(crate) fn recv_delivery(&self) -> Result<EventDelivery, RecvError> {
        self.delivery_timeout(None).map_err(|_| RecvError)
    }
    fn delivery_timeout(
        &self,
        deadline: Option<Instant>,
    ) -> Result<EventDelivery, RecvTimeoutError> {
        loop {
            match self.try_delivery() {
                Ok(event) => return Ok(event),
                Err(TryRecvError::Disconnected) => return Err(RecvTimeoutError::Disconnected),
                Err(TryRecvError::Empty) => {}
            }
            let wait = deadline.map_or(Duration::MAX, |d| {
                d.saturating_duration_since(Instant::now())
            });
            if wait.is_zero() {
                return Err(RecvTimeoutError::Timeout);
            }
            // Both producers share ownership and disconnect together. Biased selection
            // reserves callback progress for topics even when media is saturated.
            let delivery = crossbeam_channel::select_biased! {
                recv(self.control) -> event => event.map_err(|_| RecvTimeoutError::Disconnected)?,
                recv(self.media) -> event => event.map_err(|_| RecvTimeoutError::Disconnected)?,
                default(wait) => return Err(RecvTimeoutError::Timeout),
            };
            if delivery.is_current() {
                return Ok(delivery);
            }
        }
    }
    pub fn recv(&self) -> Result<CallbackEvent, RecvError> {
        self.recv_delivery()
            .map(|mut delivery| delivery.take_event())
    }
    pub fn try_recv(&self) -> Result<CallbackEvent, TryRecvError> {
        self.try_delivery()
            .map(|mut delivery| delivery.take_event())
    }
    pub fn recv_timeout(&self, timeout: Duration) -> Result<CallbackEvent, RecvTimeoutError> {
        // An unrepresentably long timeout is equivalent to no deadline.
        self.delivery_timeout(Instant::now().checked_add(timeout))
            .map(|mut delivery| delivery.take_event())
    }
}

fn json_size(value: &Value) -> usize {
    let mut size = 0usize;
    let mut pending = vec![value];
    while let Some(value) = pending.pop() {
        size = size.saturating_add(128);
        match value {
            Value::String(text) => size = size.saturating_add(text.capacity()),
            Value::Array(values) => {
                size = size.saturating_add(
                    values
                        .capacity()
                        .saturating_mul(std::mem::size_of::<Value>()),
                );
                if size > CONTROL_BYTES {
                    return usize::MAX;
                }
                pending.extend(values);
            }
            Value::Object(values) => {
                for (key, value) in values {
                    size = size.saturating_add(key.capacity()).saturating_add(128);
                    if size > CONTROL_BYTES {
                        return usize::MAX;
                    }
                    pending.push(value);
                }
            }
            _ => {}
        }
        if size > CONTROL_BYTES {
            return usize::MAX;
        }
    }
    size
}
fn event_size(event: &CallbackEvent) -> (usize, usize) {
    let base = std::mem::size_of::<CallbackEvent>();
    let (class, bytes) = match event {
        CallbackEvent::TopicCallback { topic, payload } => {
            (0, topic.capacity().saturating_add(json_size(payload)))
        }
        CallbackEvent::FutureResolve { key, result } => {
            (0, key.capacity().saturating_add(json_size(result)))
        }
        CallbackEvent::LidarCallback {
            topic,
            payload,
            points,
        } => (
            1,
            topic
                .capacity()
                .saturating_add(json_size(payload))
                .saturating_add(points.capacity().saturating_mul(4)),
        ),
        CallbackEvent::VideoFrame { data, .. } => (1, data.capacity()),
        CallbackEvent::AudioFrame { data, .. } => (1, data.capacity().saturating_mul(2)),
    };
    (class, bytes.saturating_add(base))
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::models::CallbackEvent;

    fn media(bytes: usize) -> CallbackEvent {
        CallbackEvent::VideoFrame {
            data: Vec::with_capacity(bytes),
            width: 1,
            height: 1,
        }
    }

    #[test]
    fn budgets_include_dequeued_delivery_and_old_generations() {
        let (tx, rx) = event_channel(8).unwrap();
        let old = tx.for_generation(0);
        tx.try_send(media(20 * 1024 * 1024)).unwrap();
        let delivery = rx.recv_delivery().unwrap();
        assert!(tx.try_send(media(20 * 1024 * 1024)).is_err());
        tx.set_generation(1);
        assert!(!delivery.is_current());
        assert!(old.try_send(media(1)).is_err());
        assert!(tx
            .for_generation(1)
            .try_send(media(20 * 1024 * 1024))
            .is_err());
        drop(delivery);
        assert!(tx
            .for_generation(1)
            .try_send(media(20 * 1024 * 1024))
            .is_ok());
    }
}

#[cfg(test)]
mod admission_tests {
    use super::*;
    use serde_json::json;

    fn media(bytes: usize) -> CallbackEvent {
        CallbackEvent::VideoFrame {
            data: Vec::with_capacity(bytes),
            width: 1,
            height: 1,
        }
    }
    fn topic() -> CallbackEvent {
        CallbackEvent::TopicCallback {
            topic: "state".into(),
            payload: json!({}),
        }
    }
    #[test]
    fn control_has_independent_capacity_and_priority() {
        let (tx, rx) = event_channel(1).unwrap();
        tx.try_send(media(1024)).unwrap();
        assert!(tx.try_send(media(1)).is_err());
        tx.try_send(topic()).unwrap();
        assert!(matches!(
            rx.try_recv().unwrap(),
            CallbackEvent::TopicCallback { .. }
        ));
        assert!(matches!(
            rx.try_recv().unwrap(),
            CallbackEvent::VideoFrame { .. }
        ));
        tx.try_send(media(1)).unwrap();
    }
    #[test]
    fn extracting_event_does_not_release_dispatch_permit() {
        let (tx, rx) = event_channel(1).unwrap();
        tx.try_send(media(1)).unwrap();
        let mut delivery = rx.recv_delivery().unwrap();
        let _event = delivery.take_event();
        assert!(tx.try_send(media(1)).is_err());
        drop(delivery);
        tx.try_send(media(1)).unwrap();
    }
    #[test]
    fn stale_queued_events_release_permits_and_never_surface() {
        let (tx, rx) = event_channel(1).unwrap();
        tx.try_send(media(1)).unwrap();
        tx.set_generation(1);
        assert!(matches!(rx.try_recv(), Err(TryRecvError::Empty)));
        tx.for_generation(1).try_send(media(1)).unwrap();
        assert!(rx.recv_timeout(Duration::from_millis(50)).is_ok());
    }
    #[test]
    fn failed_enqueue_and_receiver_drop_release_all_usage() {
        let (tx, rx) = event_channel(1).unwrap();
        assert!(tx.try_send(media(MEDIA_BYTES)).is_err());
        tx.try_send(media(1)).unwrap();
        drop(rx);
        assert_eq!(tx.budget.usage.lock().unwrap().count, [0, 0]);
        assert!(matches!(
            tx.try_send(media(1)),
            Err(TrySendError::Disconnected(_))
        ));
        assert_eq!(tx.budget.usage.lock().unwrap().bytes, [0, 0]);
    }
    #[test]
    fn concurrent_admission_cannot_exceed_count_or_bytes() {
        let (tx, rx) = event_channel(256).unwrap();
        std::thread::scope(|scope| {
            for _ in 0..16 {
                let tx = tx.clone();
                scope.spawn(move || {
                    let _ = tx.try_send(media(10 * 1024 * 1024));
                });
            }
        });
        let usage = tx.budget.usage.lock().unwrap();
        assert_eq!(usage.count[1], 3);
        assert!(usage.bytes[1] <= MEDIA_BYTES);
        drop(usage);
        drop(rx);
        assert_eq!(tx.budget.usage.lock().unwrap().bytes, [0, 0]);
    }
    #[test]
    fn rejects_oversized_json_and_invalid_channel_capacity() {
        assert!(event_channel(0).is_err());
        assert!(event_channel(257).is_err());
        let (tx, _rx) = event_channel(4).unwrap();
        assert!(tx
            .try_send(CallbackEvent::TopicCallback {
                topic: String::new(),
                payload: Value::String("x".repeat(CONTROL_BYTES)),
            })
            .is_err());
        assert_eq!(tx.budget.usage.lock().unwrap().bytes, [0, 0]);
    }
}

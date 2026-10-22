//! Owner-local aggregation for fixed, SDK-controlled recoverable failure classes.
use std::sync::Mutex;
use std::time::{Duration, Instant};

const WINDOW: Duration = Duration::from_secs(5);

#[derive(Default)]
struct RepeatState {
    last_report: Option<Instant>,
    pending: u64,
}

pub(crate) struct RepeatLog {
    event: &'static str,
    reason: &'static str,
    state: Mutex<RepeatState>,
}

impl RepeatLog {
    pub(crate) fn new(event: &'static str, reason: &'static str) -> Self {
        Self {
            event,
            reason,
            state: Mutex::new(RepeatState::default()),
        }
    }

    pub(crate) fn record(&self) {
        self.record_with(|count, suppressed| self.emit((count, suppressed)));
    }

    pub(crate) fn record_with(&self, emit: impl FnOnce(u64, u64)) {
        if let Some((count, suppressed)) = self.record_at(Instant::now()) {
            emit(count, suppressed);
        }
    }

    // Counts describe events not previously represented by another log line.
    fn record_at(&self, now: Instant) -> Option<(u64, u64)> {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let Some(last_report) = state.last_report else {
            state.last_report = Some(now);
            return Some((1, 0));
        };
        state.pending = state.pending.saturating_add(1);
        if now.saturating_duration_since(last_report) < WINDOW {
            return None;
        }
        state.last_report = Some(now);
        let count = std::mem::take(&mut state.pending);
        Some((count, count))
    }

    fn take_pending(&self) -> Option<(u64, u64)> {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let count = std::mem::take(&mut state.pending);
        (count != 0).then_some((count, count))
    }

    fn emit(&self, (count, suppressed_count): (u64, u64)) {
        // Format outside the state lock; no peer data or exception text is retained.
        tracing::warn!(
            event = self.event,
            reason = self.reason,
            count,
            suppressed_count
        );
    }

    pub(crate) fn flush(&self) {
        if let Some(counts) = self.take_pending() {
            self.emit(counts);
        }
    }

    pub(crate) fn reset(&self) {
        let counts = {
            let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            let count = state.pending;
            *state = RepeatState::default();
            (count != 0).then_some((count, count))
        };
        if let Some(counts) = counts {
            self.emit(counts);
        }
    }
}

impl Drop for RepeatLog {
    fn drop(&mut self) {
        // Subscriber teardown and process exit may prevent this final best-effort report.
        self.flush();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone, Default)]
    struct Output(std::sync::Arc<Mutex<Vec<u8>>>);

    impl std::io::Write for Output {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(bytes);
            Ok(bytes.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn output_is_bounded_and_drop_flushes_without_duplication() {
        if crate::test_support::run_isolated(
            "diagnostics::tests::output_is_bounded_and_drop_flushes_without_duplication",
        ) {
            return;
        }
        let output = Output::default();
        let writer = output.clone();
        let subscriber = tracing_subscriber::fmt()
            .with_ansi(false)
            .without_time()
            .with_writer(move || writer.clone())
            .finish();
        tracing::subscriber::with_default(subscriber, || {
            let log = RepeatLog::new("callback_failure", "unknown_exception");
            for _ in 0..10_000 {
                log.record();
            }
        });
        let bytes = output.0.lock().unwrap();
        let text = std::str::from_utf8(&bytes).unwrap();
        assert_eq!(text.lines().count(), 2, "{text}");
        assert!(text.contains("count=1 suppressed_count=0"), "{text}");
        assert!(text.contains("count=9999 suppressed_count=9999"), "{text}");
        assert!(text.lines().all(|line| line.len() <= 4096));
    }

    #[test]
    fn burst_summaries_account_for_every_event_and_flush_once() {
        let log = RepeatLog::new("queue_loss", "full");
        let start = Instant::now();
        assert_eq!(log.record_at(start), Some((1, 0)));
        for _ in 1..10_000 {
            assert_eq!(log.record_at(start), None);
        }
        assert_eq!(log.record_at(start + WINDOW), Some((10_000, 10_000)));
        assert_eq!(log.take_pending(), None);
        assert_eq!(log.record_at(start + WINDOW), None);
        assert_eq!(log.take_pending(), Some((1, 1)));
        assert_eq!(log.take_pending(), None);
        assert_eq!(log.record_at(start + WINDOW * 2), Some((1, 1)));
    }

    #[test]
    fn owners_are_independent_and_counters_saturate() {
        let first = RepeatLog::new("decode", "malformed");
        let second = RepeatLog::new("decode", "malformed");
        let now = Instant::now();
        assert_eq!(first.record_at(now), Some((1, 0)));
        assert_eq!(first.record_at(now), None);
        assert_eq!(second.record_at(now), Some((1, 0)));
        first.state.lock().unwrap().pending = u64::MAX;
        assert_eq!(first.record_at(now), None);
        assert_eq!(first.take_pending(), Some((u64::MAX, u64::MAX)));
        assert_eq!(second.take_pending(), None);
    }

    #[test]
    fn reset_flushes_previous_generation_and_reports_the_new_first_event() {
        let log = RepeatLog::new("queue_loss", "full");
        let now = Instant::now();
        assert_eq!(log.record_at(now), Some((1, 0)));
        assert_eq!(log.record_at(now), None);
        log.reset();
        assert_eq!(log.take_pending(), None);
        assert_eq!(log.record_at(now), Some((1, 0)));
    }

    #[test]
    fn concurrent_calls_have_one_first_event_and_exact_pending_count() {
        let log = std::sync::Arc::new(RepeatLog::new("queue_loss", "full"));
        let now = Instant::now();
        let workers: Vec<_> = (0..8)
            .map(|_| {
                let log = log.clone();
                std::thread::spawn(move || (0..1_000).filter_map(|_| log.record_at(now)).count())
            })
            .collect();
        assert_eq!(
            workers
                .into_iter()
                .map(|w| w.join().unwrap())
                .sum::<usize>(),
            1
        );
        assert_eq!(log.take_pending(), Some((7_999, 7_999)));
    }
}

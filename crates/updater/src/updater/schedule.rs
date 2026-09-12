//! When a tick is allowed to check.
//!
//! The interval predicate and the saturating backoff are the whole of what
//! generalised out of Replay's database-updater scheduling; the rest of that
//! layer was database policy and stayed behind.

use std::time::Duration;

use crate::policy::retry_delay_ms;

/// The interval between checks, stretched while attempts keep failing.
#[derive(Debug)]
pub(super) struct Schedule {
    interval_ms: i64,
    last_attempt_ms: Option<i64>,
    failures: u32,
}

impl Schedule {
    pub(super) fn new(interval: Duration) -> Self {
        Self {
            interval_ms: i64::try_from(interval.as_millis()).unwrap_or(i64::MAX),
            last_attempt_ms: None,
            failures: 0,
        }
    }

    /// Whether enough time has passed to attempt another check.
    ///
    /// A client that has never checked is due immediately: the first thing a
    /// consumer wants after a cold start is to know where it stands.
    pub(super) fn due(&self, now_ms: i64) -> bool {
        self.last_attempt_ms
            .is_none_or(|last| now_ms.saturating_sub(last) >= self.wait_ms())
    }

    pub(super) fn attempted(&mut self, now_ms: i64) {
        self.last_attempt_ms = Some(now_ms);
    }

    pub(super) fn succeeded(&mut self) {
        self.failures = 0;
    }

    pub(super) fn failed(&mut self) {
        self.failures = self.failures.saturating_add(1);
    }

    fn wait_ms(&self) -> i64 {
        match self.failures.checked_sub(1) {
            None => self.interval_ms,
            Some(prior) => self.interval_ms.max(retry_delay_ms(prior)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn schedule(interval_ms: u64) -> Schedule {
        Schedule::new(Duration::from_millis(interval_ms))
    }

    #[test]
    fn a_client_that_has_never_checked_is_due_at_once() {
        assert!(schedule(60_000).due(0));
    }

    #[test]
    fn nothing_is_due_again_until_the_interval_has_passed() {
        let mut schedule = schedule(60_000);
        schedule.attempted(1_000);

        assert!(!schedule.due(1_000));
        assert!(!schedule.due(60_999));
        assert!(schedule.due(61_000));
    }

    #[test]
    fn repeated_failure_stretches_the_wait_past_the_interval() {
        let mut schedule = schedule(1_000);
        schedule.attempted(0);
        for _ in 0..4 {
            schedule.failed();
        }

        // Four failures back off to eight seconds, well past the interval.
        assert!(!schedule.due(7_999));
        assert!(schedule.due(8_000));
    }

    #[test]
    fn a_backoff_shorter_than_the_interval_never_shortens_it() {
        let mut schedule = schedule(60_000);
        schedule.attempted(0);
        schedule.failed();

        assert!(!schedule.due(59_999));
        assert!(schedule.due(60_000));
    }

    #[test]
    fn a_runaway_failure_count_saturates_rather_than_overflowing() {
        let mut schedule = schedule(1_000);
        schedule.attempted(0);
        for _ in 0..64 {
            schedule.failed();
        }

        assert!(schedule.wait_ms() > 0);
        assert!(schedule.due(i64::MAX));
    }

    #[test]
    fn a_success_returns_the_schedule_to_its_interval() {
        let mut schedule = schedule(1_000);
        schedule.attempted(0);
        for _ in 0..4 {
            schedule.failed();
        }
        schedule.succeeded();

        assert!(schedule.due(1_000));
    }
}

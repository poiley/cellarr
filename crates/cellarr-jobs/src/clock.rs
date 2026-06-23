//! A pluggable clock so the scheduler is testable without real sleeps.
//!
//! Scheduling is "fire at time T"; retry/backoff is "wait D then retry". Both
//! are pure functions of *time*, so the scheduler reads the current instant from
//! a [`Clock`] rather than the wall clock. Production injects [`SystemClock`];
//! tests inject [`LogicalClock`] and advance it explicitly — no `tokio::time`
//! pausing, no flaky `sleep`s, and the exact retry schedule is asserted by
//! stepping the clock and observing what the scheduler decides to run.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

/// A monotonic source of "now", in whole seconds since an arbitrary epoch.
///
/// Seconds (not `Instant`) because schedules and backoff windows are expressed
/// in seconds and a logical clock must be trivially serializable for the
/// persistence tests. The epoch is unspecified; only differences are meaningful.
pub trait Clock: Send + Sync {
    /// The current time, in seconds since the clock's epoch.
    fn now_secs(&self) -> u64;
}

/// The production clock: wall-clock seconds since the Unix epoch.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now_secs(&self) -> u64 {
        time::OffsetDateTime::now_utc().unix_timestamp().max(0) as u64
    }
}

/// A manually-advanced clock for deterministic tests.
///
/// Cloning shares the same underlying time, so a test holds one handle and the
/// scheduler another, and [`LogicalClock::advance`] moves both. There are no
/// real sleeps anywhere in the scheduler's time logic.
#[derive(Debug, Clone, Default)]
pub struct LogicalClock {
    now: Arc<AtomicU64>,
}

impl LogicalClock {
    /// A logical clock starting at `start` seconds.
    #[must_use]
    pub fn new(start: u64) -> Self {
        Self {
            now: Arc::new(AtomicU64::new(start)),
        }
    }

    /// Advance the clock by `secs` seconds and return the new time.
    pub fn advance(&self, secs: u64) -> u64 {
        self.now.fetch_add(secs, Ordering::SeqCst) + secs
    }

    /// Set the clock to an absolute time.
    pub fn set(&self, secs: u64) {
        self.now.store(secs, Ordering::SeqCst);
    }
}

impl Clock for LogicalClock {
    fn now_secs(&self) -> u64 {
        self.now.load(Ordering::SeqCst)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn logical_clock_advances_and_is_shared_across_clones() {
        let a = LogicalClock::new(100);
        let b = a.clone();
        assert_eq!(a.now_secs(), 100);
        assert_eq!(b.advance(25), 125);
        // The clone shares state: advancing one moves the other.
        assert_eq!(a.now_secs(), 125);
        a.set(0);
        assert_eq!(b.now_secs(), 0);
    }
}

//! Per-host rate limiting.
//!
//! Indexers ban aggressively, so every outbound request is gated by a
//! conservative per-host limiter. Keying on host (not on the configured indexer)
//! means several indexers sharing one tracker host still share one budget, which
//! is what trackers actually enforce (`docs/06-integrations.md`).

use std::num::NonZeroU32;
use std::time::Duration;

use governor::{DefaultKeyedRateLimiter, Quota};

/// A conservative, host-keyed rate limiter shared across indexer instances.
pub struct HostRateLimiter {
    limiter: DefaultKeyedRateLimiter<String>,
}

impl HostRateLimiter {
    /// Build a limiter allowing `max_burst` requests, replenishing one slot every
    /// `per` duration, independently per host.
    ///
    /// Returns `None` only if the supplied period is zero, which would make the
    /// quota meaningless.
    #[must_use]
    pub fn new(max_burst: NonZeroU32, per: Duration) -> Option<Self> {
        let quota = Quota::with_period(per)?.allow_burst(max_burst);
        Some(Self {
            limiter: DefaultKeyedRateLimiter::keyed(quota),
        })
    }

    /// A sane default: a small burst, then ~one request per second per host.
    #[must_use]
    pub fn conservative_default() -> Self {
        // Safe: both literals are non-zero and the period is non-zero, so the
        // quota construction below cannot fail.
        let burst = NonZeroU32::new(5).expect("5 is non-zero");
        let per = Duration::from_secs(1);
        Self::new(burst, per).expect("non-zero period yields a valid quota")
    }

    /// Wait until a request to `host` is permitted, then return.
    pub async fn until_ready(&self, host: &str) {
        self.limiter.until_key_ready(&host.to_string()).await;
    }

    /// Non-blocking check: `true` if a request to `host` may proceed now.
    #[must_use]
    pub fn check(&self, host: &str) -> bool {
        self.limiter.check_key(&host.to_string()).is_ok()
    }
}

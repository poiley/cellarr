//! Per-source rate limiting.
//!
//! Each source has its own conservative ceiling (`docs/07-metadata-service.md`):
//! MusicBrainz ~1 req/s, AniDB 1 req/2s with aggressive bans, TMDb soft tens of
//! req/s, TheTVDB unpublished → conservative. We wrap `governor` so an adapter
//! just calls [`RateLimiter::until_ready`] before each request and the limiter
//! enforces the source's quota, blocking (asynchronously) when needed.
//!
//! The limiter is a no-op cost on the cached path — adapters consult the cache
//! first and only acquire a permit on a real network call.

use std::num::NonZeroU32;

use governor::clock::DefaultClock;
use governor::state::{InMemoryState, NotKeyed};
use governor::{Quota, RateLimiter as Governor};

/// An async, per-source request limiter.
pub struct RateLimiter {
    inner: Governor<NotKeyed, InMemoryState, DefaultClock>,
}

impl RateLimiter {
    /// A limiter allowing at most `per_minute` requests per minute.
    ///
    /// `per_minute` is clamped to at least 1; a zero quota would deadlock the
    /// daemon, which is never the intent for a "conservative" limit.
    #[must_use]
    pub fn per_minute(per_minute: u32) -> Self {
        let n = NonZeroU32::new(per_minute.max(1)).unwrap_or(NonZeroU32::MIN);
        Self {
            inner: Governor::direct(Quota::per_minute(n)),
        }
    }

    /// A limiter allowing at most `per_second` requests per second.
    #[must_use]
    pub fn per_second(per_second: u32) -> Self {
        let n = NonZeroU32::new(per_second.max(1)).unwrap_or(NonZeroU32::MIN);
        Self {
            inner: Governor::direct(Quota::per_second(n)),
        }
    }

    /// Await until a request permit is available, then consume it.
    pub async fn until_ready(&self) {
        self.inner.until_ready().await;
    }

    /// Try to consume a permit without waiting; `true` if one was available.
    ///
    /// Useful for tests and for callers that prefer to serve stale-but-present
    /// cache data rather than block on the limiter.
    pub fn check(&self) -> bool {
        self.inner.check().is_ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fresh_limiter_admits_then_blocks_within_window() {
        // One per minute: the first check passes, the immediate second fails.
        let rl = RateLimiter::per_minute(1);
        assert!(rl.check());
        assert!(!rl.check());
    }

    #[tokio::test]
    async fn until_ready_returns_immediately_when_permit_available() {
        let rl = RateLimiter::per_second(10);
        // Should not hang; a permit is available on a fresh limiter.
        rl.until_ready().await;
    }

    #[test]
    fn zero_quota_is_clamped_to_one() {
        // A zero quota would deadlock; it must clamp to at least one permit.
        let rl = RateLimiter::per_second(0);
        assert!(rl.check());
    }
}

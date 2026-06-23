//! Rate-limiter behavior: a small burst is allowed, then requests are throttled.

use std::num::NonZeroU32;
use std::time::Duration;

use cellarr_indexers::HostRateLimiter;

#[test]
fn allows_burst_then_throttles_per_host() {
    // 2-request burst, replenishing one slot per (long) period so the burst is
    // exhausted deterministically within the test.
    let burst = NonZeroU32::new(2).expect("non-zero");
    let limiter = HostRateLimiter::new(burst, Duration::from_secs(3600)).expect("valid quota");

    assert!(limiter.check("host-a"), "first request allowed");
    assert!(limiter.check("host-a"), "second request allowed (burst)");
    assert!(!limiter.check("host-a"), "third request throttled");

    // A different host has its own independent budget.
    assert!(limiter.check("host-b"), "other host is independent");
}

#[tokio::test]
async fn until_ready_resolves_within_burst() {
    let burst = NonZeroU32::new(1).expect("non-zero");
    let limiter = HostRateLimiter::new(burst, Duration::from_secs(3600)).expect("valid quota");
    // The first call resolves immediately (slot available); this just asserts the
    // async path is wired and does not hang for an available slot.
    limiter.until_ready("host").await;
}

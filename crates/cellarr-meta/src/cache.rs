//! The in-process metadata cache.
//!
//! Metadata changes slowly, so a long-TTL cache cuts source load and latency
//! dramatically (`docs/07-metadata-service.md`) and is what lets the daemon keep
//! serving when a source is briefly unreachable. This is the **in-process
//! `moka`** layer with per-source TTLs; a persisted cache table via `cellarr-db`
//! is a deliberate follow-up and is *not* a dependency here.
//!
//! Cache keys are strings the adapter composes (`"tmdb:fetch:603"`,
//! `"tvdb:search:firefly"`), values are the raw JSON bodies. Storing raw bodies
//! (not normalized structs) means a normalization change does not invalidate the
//! cache, and the same entry serves both `serde_json::Value` consumers and the
//! normalizers.
//!
//! Stampede protection: [`MetaCache::get_or_try_insert_with`] uses moka's
//! coalescing loader so N concurrent misses for the same key trigger exactly one
//! upstream fetch.

use std::future::Future;
use std::time::Duration;

use moka::future::Cache;

use crate::error::MetaError;

/// An in-process cache of raw source responses, with a fixed TTL.
#[derive(Clone)]
pub struct MetaCache {
    inner: Cache<String, String>,
}

impl MetaCache {
    /// Build a cache that expires entries `ttl` after they are written and holds
    /// at most `capacity` entries.
    #[must_use]
    pub fn new(ttl: Duration, capacity: u64) -> Self {
        let inner = Cache::builder()
            .max_capacity(capacity)
            .time_to_live(ttl)
            .build();
        Self { inner }
    }

    /// Fetch a cached value without triggering a load.
    pub async fn get(&self, key: &str) -> Option<String> {
        self.inner.get(key).await
    }

    /// Insert a value under `key`.
    pub async fn insert(&self, key: String, value: String) {
        self.inner.insert(key, value).await;
    }

    /// Return the cached value for `key`, or run `init` to produce it and cache
    /// the result. Concurrent callers for the same key share one `init` run
    /// (stampede protection); a failing `init` is **not** cached, so a transient
    /// source error does not poison the entry.
    ///
    /// # Errors
    /// Propagates the error from `init` when the value is not cached and the load
    /// fails.
    pub async fn get_or_try_insert_with<F>(&self, key: &str, init: F) -> Result<String, MetaError>
    where
        F: Future<Output = Result<String, MetaError>>,
    {
        self.inner
            .try_get_with(key.to_string(), init)
            .await
            // moka wraps the loader error in an Arc shared across coalesced
            // callers and keeps a reference of its own, so we clone out the
            // original (typed) error rather than try to take ownership.
            .map_err(|arc: std::sync::Arc<MetaError>| (*arc).clone())
    }

    /// Force any pending eviction/expiry bookkeeping to run. Tests call this
    /// after advancing time so TTL assertions are deterministic.
    pub async fn sync(&self) {
        self.inner.run_pending_tasks().await;
    }

    /// The number of entries currently held (after pending tasks run).
    pub async fn entry_count(&self) -> u64 {
        self.inner.run_pending_tasks().await;
        self.inner.entry_count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    #[tokio::test]
    async fn caches_loaded_value_and_loads_once() {
        let cache = MetaCache::new(Duration::from_secs(60), 100);
        let calls = Arc::new(AtomicUsize::new(0));

        for _ in 0..3 {
            let calls = calls.clone();
            let v = cache
                .get_or_try_insert_with("k", async move {
                    calls.fetch_add(1, Ordering::SeqCst);
                    Ok("v".to_string())
                })
                .await
                .unwrap();
            assert_eq!(v, "v");
        }
        // Three reads, one load: the cache served the last two.
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn failing_loader_is_not_cached() {
        let cache = MetaCache::new(Duration::from_secs(60), 100);
        let err = cache
            .get_or_try_insert_with("k", async {
                Err(MetaError::Http {
                    src: "tmdb",
                    status: 429,
                })
            })
            .await
            .unwrap_err();
        assert!(matches!(err, MetaError::Http { status: 429, .. }));
        // A transient error must not poison the entry: a later success caches.
        let v = cache
            .get_or_try_insert_with("k", async { Ok("recovered".to_string()) })
            .await
            .unwrap();
        assert_eq!(v, "recovered");
    }

    #[tokio::test]
    async fn entries_expire_after_ttl() {
        let cache = MetaCache::new(Duration::from_millis(50), 100);
        cache.insert("k".to_string(), "v".to_string()).await;
        assert_eq!(cache.entry_count().await, 1);
        tokio::time::sleep(Duration::from_millis(120)).await;
        cache.sync().await;
        assert!(cache.get("k").await.is_none());
    }
}

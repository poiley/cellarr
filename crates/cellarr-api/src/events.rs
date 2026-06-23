//! Live push events (Server-Sent Events).
//!
//! The native API pushes updates instead of making clients poll (docs/09-api.md).
//! Events are published on a [`tokio::sync::broadcast`] channel by the domain
//! code that performs the transition — a queue progress change, an import, a new
//! decision-log entry — and fanned out to every connected `/api/v1/stream`
//! subscriber. There is **no polling timer**: an event exists only because a
//! real domain transition produced it via [`EventBus::publish`].

use serde::Serialize;
use tokio::sync::broadcast;

/// A single live event delivered over the stream. Tagged JSON so a client can
/// switch on `type`.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum DomainEvent {
    /// A queued grab made progress (download client reported new state).
    QueueProgress {
        /// The grab whose progress changed.
        grab_id: String,
        /// The grab's lifecycle status after the transition.
        status: String,
        /// Download progress in `[0.0, 1.0]`, when the client reports it.
        #[serde(skip_serializing_if = "Option::is_none")]
        progress: Option<f32>,
    },
    /// A file was imported into the library.
    ImportCompleted {
        /// The content node the file was imported for.
        content_id: String,
        /// The imported file's path.
        path: String,
    },
    /// A new decision-log entry was appended (a grab/reject/upgrade verdict).
    DecisionLogged {
        /// The pipeline run the decision belongs to.
        run_id: String,
        /// A short human summary of the transition.
        note: String,
    },
    /// A command (search/import/refresh) was accepted by the scheduler.
    CommandQueued {
        /// The scheduler job id.
        job_id: String,
        /// The command name.
        name: String,
    },
}

/// The publish/subscribe seam for live events.
///
/// Cloneable and cheap; handed to every component that performs a domain
/// transition (the pipeline, the queue poller, the command endpoints) so they
/// can `publish`, and to the stream handler so it can `subscribe`. A lagging or
/// disconnected subscriber never blocks a publisher — `broadcast` drops the
/// slowest receiver's oldest messages instead.
#[derive(Clone)]
pub struct EventBus {
    tx: broadcast::Sender<DomainEvent>,
}

impl EventBus {
    /// Create a bus with the given channel capacity (per-subscriber backlog).
    #[must_use]
    pub fn new(capacity: usize) -> Self {
        let (tx, _rx) = broadcast::channel(capacity);
        Self { tx }
    }

    /// Publish a domain event to all current subscribers.
    ///
    /// Returns the number of subscribers it reached. A return of zero (no
    /// listeners) is normal and not an error.
    pub fn publish(&self, event: DomainEvent) -> usize {
        // `send` errors only when there are no receivers; that is expected.
        self.tx.send(event).unwrap_or(0)
    }

    /// Subscribe to the stream. Each subscriber gets its own receiver.
    #[must_use]
    pub fn subscribe(&self) -> broadcast::Receiver<DomainEvent> {
        self.tx.subscribe()
    }
}

impl Default for EventBus {
    fn default() -> Self {
        // A modest backlog: enough to absorb a burst of queue updates without
        // unbounded memory, small enough that a stuck client is cheap.
        Self::new(256)
    }
}

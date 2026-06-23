//! The live `/api/v1/stream` Server-Sent Events endpoint.
//!
//! Subscribes the client to the [`EventBus`](crate::events::EventBus) and
//! forwards each [`DomainEvent`](crate::events::DomainEvent) as an SSE message.
//! The stream is driven entirely by real domain transitions published on the
//! bus — there is no polling timer. A periodic keep-alive comment keeps idle
//! connections (and proxies) from dropping; it carries no data.

use std::convert::Infallible;
use std::time::Duration;

use axum::extract::State;
use axum::response::sse::{Event, KeepAlive, Sse};
use futures::Stream;
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::StreamExt;

use crate::state::AppState;

/// Open an SSE stream of live domain events for one client.
pub async fn sse(
    State(state): State<AppState>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let rx = state.events.subscribe();
    let stream = BroadcastStream::new(rx).filter_map(|item| {
        // A lagged receiver (slow client) yields an error; skip it rather than
        // closing the stream, so a brief stall never disconnects the client.
        let event = item.ok()?;
        // Serialization of our own owned types cannot fail in practice; on the
        // impossible error, skip the frame rather than tear down the stream.
        let json = serde_json::to_string(&event).ok()?;
        Some(Ok(Event::default().event(event_name(&event)).data(json)))
    });

    Sse::new(stream).keep_alive(
        KeepAlive::new()
            .interval(Duration::from_secs(15))
            .text("keep-alive"),
    )
}

/// The SSE `event:` name for a domain event, so clients can `addEventListener`
/// per type. Mirrors the serde tag.
fn event_name(event: &crate::events::DomainEvent) -> &'static str {
    use crate::events::DomainEvent::*;
    match event {
        QueueProgress { .. } => "queue_progress",
        ImportCompleted { .. } => "import_completed",
        DecisionLogged { .. } => "decision_logged",
        CommandQueued { .. } => "command_queued",
    }
}

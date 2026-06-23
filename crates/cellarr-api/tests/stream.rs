//! Live-push (SSE) test.
//!
//! Asserts that an event is pushed on a **real domain transition** — submitting
//! a command — and not on any timer. The test opens the `/api/v1/stream` SSE
//! endpoint, triggers a command over the API, and reads the stream until the
//! matching `command_queued` event arrives.

mod common;

use std::time::Duration;

use common::start_open;
use futures::StreamExt;

#[tokio::test]
async fn command_transition_pushes_an_sse_event() {
    let server = start_open().await;

    // Open the SSE stream first so we are subscribed before the transition.
    let resp = server
        .client()
        .get(server.url("/api/v1/stream"))
        .header("accept", "text/event-stream")
        .send()
        .await
        .expect("open stream");
    assert_eq!(resp.status(), 200);
    assert_eq!(
        resp.headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok()),
        Some("text/event-stream")
    );

    let mut body = resp.bytes_stream();

    // Drive a real domain transition: submit a command. We do this from a
    // spawned task after a tiny delay so the reader is already polling.
    let trigger_url = server.url("/api/v1/commands");
    let client = server.client();
    tokio::spawn(async move {
        // A short yield so the SSE subscription is registered first.
        tokio::time::sleep(Duration::from_millis(50)).await;
        let _ = client
            .post(trigger_url)
            .json(&serde_json::json!({ "name": "RssSync" }))
            .send()
            .await;
    });

    // Read frames until we see the command event or time out.
    let mut buf = String::new();
    let got = tokio::time::timeout(Duration::from_secs(5), async {
        while let Some(chunk) = body.next().await {
            let chunk = chunk.expect("stream chunk");
            buf.push_str(&String::from_utf8_lossy(&chunk));
            if buf.contains("command_queued") && buf.contains("RssSync") {
                return true;
            }
        }
        false
    })
    .await
    .expect("timed out waiting for SSE event");

    assert!(
        got,
        "did not receive command_queued event; buffer was: {buf}"
    );
    // The frame carries the SSE event name and JSON data.
    assert!(buf.contains("event: command_queued") || buf.contains("event:command_queued"));
}

#[tokio::test]
async fn import_event_published_on_bus_reaches_subscriber() {
    // Asserts the bus delivers a non-command transition too: an import event
    // published directly (as the pipeline would) reaches an SSE subscriber.
    let server = start_open().await;
    let resp = server
        .client()
        .get(server.url("/api/v1/stream"))
        .send()
        .await
        .expect("open stream");
    let mut body = resp.bytes_stream();

    let events = server.state.events.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(50)).await;
        events.publish(cellarr_api::DomainEvent::ImportCompleted {
            content_id: "abc".into(),
            path: "/data/movie.mkv".into(),
        });
    });

    let mut buf = String::new();
    let got = tokio::time::timeout(Duration::from_secs(5), async {
        while let Some(chunk) = body.next().await {
            let chunk = chunk.expect("chunk");
            buf.push_str(&String::from_utf8_lossy(&chunk));
            if buf.contains("import_completed") {
                return true;
            }
        }
        false
    })
    .await
    .expect("timed out");
    assert!(got, "import event not received; buffer: {buf}");
}

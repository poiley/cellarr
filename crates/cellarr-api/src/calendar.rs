//! The iCal / ICS calendar feed (`/feed/v3/calendar/{sonarr,radarr}.ics`).
//!
//! The *arr ecosystem (and Google/Apple Calendar, Home Assistant, dashboards)
//! subscribes to a Sonarr/Radarr **iCal feed** to see upcoming/aired episodes and
//! movie release dates. cellarr serves the same: an `.ics` document with one
//! `VEVENT` per dated item, on a Sonarr face (TV) and a Radarr face (movies),
//! authenticated by the `apikey` query parameter the ecosystem appends to the feed
//! URL (calendar clients cannot send a header).
//!
//! Two layers:
//! - a **pure ICS writer** ([`IcsCalendar`]/[`CalendarEvent`]) that emits a
//!   spec-valid `VCALENDAR`/`VEVENT` document (CRLF line endings, escaping, line
//!   folding) — the load-bearing, fully-unit-tested part; and
//! - a **feed handler** ([`calendar_feed`]) that collects [`CalendarEvent`]s from
//!   the library (any content node whose coordinates carry an air/release date)
//!   and renders them for the addressed face.

use axum::extract::{Path, Query, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use serde::Deserialize;

use cellarr_core::repo::ContentRepository;
use cellarr_core::{Coordinates, MediaType};

use crate::state::AppState;

/// One calendar event: an all-day VEVENT for a dated library item.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CalendarEvent {
    /// A stable unique id for the event (the `UID` property). Calendar clients
    /// de-duplicate on this across refreshes.
    pub uid: String,
    /// The event date in ISO `yyyy-mm-dd` form (an all-day `VALUE=DATE` event,
    /// which is how the originals model air/release dates).
    pub date: String,
    /// The event title (the `SUMMARY` property), e.g. `"The Show - S01E02"` or a
    /// movie title.
    pub summary: String,
    /// An optional longer description (the `DESCRIPTION` property).
    pub description: Option<String>,
}

/// A pure ICS document builder. Holds events and renders a spec-valid
/// `VCALENDAR` with `VEVENT`s.
#[derive(Debug, Default)]
pub struct IcsCalendar {
    events: Vec<CalendarEvent>,
}

impl IcsCalendar {
    /// An empty calendar.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Add an event.
    pub fn push(&mut self, event: CalendarEvent) {
        self.events.push(event);
    }

    /// The number of events.
    #[must_use]
    pub fn len(&self) -> usize {
        self.events.len()
    }

    /// Whether the calendar has no events.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }

    /// Render the full `VCALENDAR` text. RFC 5545: CRLF line endings, a
    /// `VCALENDAR` wrapper with `VERSION`/`PRODID`, and one `VEVENT` per event
    /// with an all-day `DTSTART;VALUE=DATE`. `SUMMARY`/`DESCRIPTION` are escaped
    /// and long lines folded.
    #[must_use]
    pub fn render(&self, prodid: &str) -> String {
        let mut out = String::new();
        push_line(&mut out, "BEGIN:VCALENDAR");
        push_line(&mut out, "VERSION:2.0");
        push_line(&mut out, &format!("PRODID:{}", escape_text(prodid)));
        push_line(&mut out, "CALSCALE:GREGORIAN");
        push_line(&mut out, "METHOD:PUBLISH");
        for ev in &self.events {
            push_line(&mut out, "BEGIN:VEVENT");
            push_line(&mut out, &format!("UID:{}", escape_text(&ev.uid)));
            // A stable DTSTAMP is fine for a published feed; use the event date at
            // midnight UTC so the document is deterministic and valid.
            push_line(
                &mut out,
                &format!("DTSTAMP:{}T000000Z", ev.date.replace('-', "")),
            );
            push_line(
                &mut out,
                &format!("DTSTART;VALUE=DATE:{}", ev.date.replace('-', "")),
            );
            fold_line(&mut out, &format!("SUMMARY:{}", escape_text(&ev.summary)));
            if let Some(desc) = &ev.description {
                fold_line(&mut out, &format!("DESCRIPTION:{}", escape_text(desc)));
            }
            push_line(&mut out, "END:VEVENT");
        }
        push_line(&mut out, "END:VCALENDAR");
        out
    }
}

/// Append a line with the required CRLF terminator.
fn push_line(out: &mut String, line: &str) {
    out.push_str(line);
    out.push_str("\r\n");
}

/// Append a content line, folding it at 75 octets per RFC 5545 (a continued line
/// starts with a single space). Folds on a simple byte boundary, which is correct
/// for the ASCII content we emit.
fn fold_line(out: &mut String, line: &str) {
    const LIMIT: usize = 75;
    if line.len() <= LIMIT {
        push_line(out, line);
        return;
    }
    let bytes = line.as_bytes();
    let mut start = 0;
    let mut first = true;
    while start < bytes.len() {
        let take = if first { LIMIT } else { LIMIT - 1 };
        let end = (start + take).min(bytes.len());
        if !first {
            out.push(' ');
        }
        // `line` is ASCII here (escaped); slicing on a byte boundary is safe.
        out.push_str(&line[start..end]);
        out.push_str("\r\n");
        start = end;
        first = false;
    }
}

/// Escape a text value per RFC 5545 (`\`, `;`, `,`, and newlines).
fn escape_text(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            ';' => out.push_str("\\;"),
            ',' => out.push_str("\\,"),
            '\n' => out.push_str("\\n"),
            '\r' => {}
            _ => out.push(c),
        }
    }
    out
}

/// The face a calendar feed serves.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FeedFace {
    /// TV episodes (the Sonarr feed).
    Sonarr,
    /// Movie releases (the Radarr feed).
    Radarr,
}

impl FeedFace {
    fn media_type(self) -> MediaType {
        match self {
            FeedFace::Sonarr => MediaType::Tv,
            FeedFace::Radarr => MediaType::Movie,
        }
    }

    fn prodid(self) -> &'static str {
        match self {
            FeedFace::Sonarr => "-//cellarr//Sonarr Calendar//EN",
            FeedFace::Radarr => "-//cellarr//Radarr Calendar//EN",
        }
    }
}

/// The `apikey` query parameter calendar clients append to the feed URL (they
/// cannot send an `X-Api-Key` header).
#[derive(Debug, Deserialize, Default)]
pub struct FeedAuthQuery {
    #[serde(default)]
    apikey: Option<String>,
}

/// The iCal feed handler. Mounted (by [`crate::build_router`]) at
/// `/feed/v3/calendar/sonarr.ics` and `…/radarr.ics`. Authenticated by the
/// `apikey` query parameter (constant-time, via the shared [`crate::AuthConfig`]).
pub async fn calendar_feed(
    State(state): State<AppState>,
    Path(file): Path<String>,
    Query(q): Query<FeedAuthQuery>,
) -> Response {
    // Auth: calendar clients can only send the apikey in the query string.
    if !state.auth.accepts(q.apikey.as_deref()) {
        return (StatusCode::UNAUTHORIZED, "unauthorized").into_response();
    }

    let face = match file.as_str() {
        "sonarr.ics" => FeedFace::Sonarr,
        "radarr.ics" => FeedFace::Radarr,
        _ => return (StatusCode::NOT_FOUND, "unknown calendar feed").into_response(),
    };

    let calendar = match build_calendar(&state, face).await {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!(error = %e, "calendar feed build failed");
            return (StatusCode::INTERNAL_SERVER_ERROR, "calendar build failed").into_response();
        }
    };

    let body = calendar.render(face.prodid());
    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "text/calendar; charset=utf-8"),
            (
                header::CONTENT_DISPOSITION,
                "inline; filename=\"cellarr.ics\"",
            ),
        ],
        body,
    )
        .into_response()
}

/// Collect calendar events from the library for `face` and build an [`IcsCalendar`].
///
/// Walks each library of the face's media type and emits an all-day VEVENT for
/// every node whose coordinates carry an air/release date. TV daily-coded episodes
/// (`Coordinates::Daily { date }`) are the dated TV items; a movie node carrying a
/// `Daily`-style date in its coords is treated likewise. Nodes without a resolvable
/// date are simply omitted (an undated item is not a calendar entry), which keeps
/// the feed valid and non-misleading.
async fn build_calendar(
    state: &AppState,
    face: FeedFace,
) -> Result<IcsCalendar, cellarr_db::DbError> {
    let media = face.media_type();
    let content = state.db.content();
    let mut cal = IcsCalendar::new();

    for lib in state.db.config().list_libraries().await? {
        if lib.media_type != media {
            continue;
        }
        // Walk the library's tree (roots then their descendants) collecting dated
        // nodes. Bounded by the library size; no recursion explosion (a tree).
        let mut stack = content.roots(lib.id).await?;
        while let Some(node) = stack.pop() {
            if let Some(event) = event_for_node(state, &node).await? {
                cal.push(event);
            }
            stack.extend(content.children(node.id).await?);
        }
    }
    Ok(cal)
}

/// Build a [`CalendarEvent`] for a node, or `None` when it carries no date.
async fn event_for_node(
    state: &AppState,
    node: &cellarr_core::ContentNode,
) -> Result<Option<CalendarEvent>, cellarr_db::DbError> {
    let date = match &node.coords {
        Coordinates::Daily { date } => date.clone(),
        // Other coordinate variants do not carry a self-contained date in the
        // current model; once the identify pipeline persists per-episode air dates
        // / movie release dates, this match gains those arms. Undated -> skip.
        _ => return Ok(None),
    };
    let title = state
        .db
        .content()
        .title_for(node.id)
        .await?
        .unwrap_or_else(|| node.id.to_string());
    let summary = match node.coords {
        Coordinates::Episode {
            season, episode, ..
        } => {
            format!("{title} - S{season:02}E{episode:02}")
        }
        _ => title.clone(),
    };
    Ok(Some(CalendarEvent {
        uid: format!("{}@cellarr", node.id),
        date,
        summary,
        description: None,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_valid_vcalendar_with_vevents() {
        let mut cal = IcsCalendar::new();
        cal.push(CalendarEvent {
            uid: "abc@cellarr".into(),
            date: "2026-07-04".into(),
            summary: "The Show - S01E02".into(),
            description: Some("A test episode".into()),
        });
        let out = cal.render("-//cellarr//test//EN");

        // CRLF line endings throughout.
        assert!(out.contains("\r\n"));
        assert!(!out.contains("\n\n"));
        // The wrapper.
        assert!(out.starts_with("BEGIN:VCALENDAR\r\n"));
        assert!(out.contains("VERSION:2.0\r\n"));
        assert!(out.trim_end().ends_with("END:VCALENDAR"));
        // One VEVENT with the expected properties.
        assert_eq!(out.matches("BEGIN:VEVENT\r\n").count(), 1);
        assert_eq!(out.matches("END:VEVENT\r\n").count(), 1);
        assert!(out.contains("UID:abc@cellarr\r\n"));
        assert!(out.contains("DTSTART;VALUE=DATE:20260704\r\n"));
        assert!(out.contains("SUMMARY:The Show - S01E02\r\n"));
        assert!(out.contains("DESCRIPTION:A test episode\r\n"));
    }

    #[test]
    fn escapes_special_characters_in_summary() {
        let mut cal = IcsCalendar::new();
        cal.push(CalendarEvent {
            uid: "x".into(),
            date: "2026-01-01".into(),
            summary: "A; B, C \\ D".into(),
            description: None,
        });
        let out = cal.render("p");
        assert!(out.contains("SUMMARY:A\\; B\\, C \\\\ D\r\n"));
    }

    #[test]
    fn folds_long_lines_at_75_octets() {
        let long = "X".repeat(200);
        let mut cal = IcsCalendar::new();
        cal.push(CalendarEvent {
            uid: "x".into(),
            date: "2026-01-01".into(),
            summary: long,
            description: None,
        });
        let out = cal.render("p");
        // Every physical line is at most 75 octets (continuations begin with a
        // space).
        for line in out.split("\r\n") {
            assert!(line.len() <= 75, "line too long: {} octets", line.len());
        }
        // The folded continuation marker (a leading space) is present.
        assert!(out.contains("\r\n "));
    }

    #[test]
    fn empty_calendar_is_still_valid() {
        let cal = IcsCalendar::new();
        assert!(cal.is_empty());
        let out = cal.render("p");
        assert!(out.contains("BEGIN:VCALENDAR\r\n"));
        assert!(out.contains("END:VCALENDAR\r\n"));
        assert_eq!(out.matches("BEGIN:VEVENT").count(), 0);
    }
}

//! Column (de)serialization helpers.
//!
//! Identifiers and timestamps are stored as TEXT for cross-engine portability
//! (see the migration header). These helpers centralize the parse/format so a
//! malformed stored value surfaces as a typed [`DbError::Decode`] rather than a
//! panic, honoring the "no unwrap on fallible runtime paths" rule.

use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::error::DbError;

/// Parse a UUID stored as TEXT.
pub(crate) fn parse_uuid(column: &'static str, s: &str) -> Result<Uuid, DbError> {
    Uuid::parse_str(s).map_err(|e| DbError::Decode {
        column,
        detail: e.to_string(),
    })
}

/// Parse an RFC3339 timestamp stored as TEXT.
pub(crate) fn parse_time(column: &'static str, s: &str) -> Result<OffsetDateTime, DbError> {
    OffsetDateTime::parse(s, &Rfc3339).map_err(|e| DbError::Decode {
        column,
        detail: e.to_string(),
    })
}

/// Format a timestamp for storage as TEXT (RFC3339, UTC).
pub(crate) fn format_time(t: OffsetDateTime) -> Result<String, DbError> {
    t.format(&Rfc3339).map_err(|e| DbError::Decode {
        column: "timestamp",
        detail: e.to_string(),
    })
}

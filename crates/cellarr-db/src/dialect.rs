//! Compile-time backend selection.
//!
//! cellarr compiles to exactly **one** database engine. By default that is
//! **SQLite** — the offline, zero-service, single-static-binary default the
//! project is built around. Under the `postgres` cargo feature it is instead
//! **Postgres**: a database *server* (e.g. one running on a NAS) reached over
//! TCP, which does its own local disk I/O and returns result sets over the wire
//! rather than serving raw pages over a network filesystem.
//!
//! The choice is made at build time so each path uses sqlx's **native** driver.
//! The proven SQLite path is never routed through a runtime abstraction layer
//! (`sqlx::Any`), so it carries neither the overhead nor the behavioural risk of
//! one: with the default feature set the query text and types are byte-for-byte
//! what they have always been.
//!
//! Everything engine-specific in the crate keys off the aliases and [`pq`] here,
//! so the repositories read as one backend-agnostic implementation.

/// The compiled connection pool type.
#[cfg(not(feature = "postgres"))]
pub use sqlx::SqlitePool as DbPool;
/// The compiled row type rows decode from.
#[cfg(not(feature = "postgres"))]
pub use sqlx::sqlite::SqliteRow as DbRow;
/// The compiled single-connection type the writer-actor drives.
#[cfg(not(feature = "postgres"))]
pub use sqlx::sqlite::SqliteConnection as DbConnection;

#[cfg(feature = "postgres")]
pub use sqlx::PgPool as DbPool;
#[cfg(feature = "postgres")]
pub use sqlx::postgres::PgRow as DbRow;
#[cfg(feature = "postgres")]
pub use sqlx::postgres::PgConnection as DbConnection;

/// Translate a SQL string authored in SQLite's `?N` positional-parameter style
/// into the compiled backend's placeholder dialect.
///
/// Under SQLite this is the **identity** function — the query text is returned
/// unchanged, so the long-proven SQLite statements are byte-for-byte what they
/// were. Under Postgres it rewrites each `?N` token to `$N` (Postgres's
/// positional style). The rewrite is purely lexical and only touches
/// `?`-followed-by-digits sequences; no query in the crate reuses a numbered
/// bind, so a straight token swap preserves bind order on both engines.
///
/// Call sites uniformly pass `&pq("…")`: under SQLite that is `&&str` (deref-
/// coerced to `&str`), under Postgres `&String` — both satisfy
/// [`sqlx::query`]'s `&str` argument. The returned/borrowed value lives to the
/// end of the enclosing statement, which is where the query is executed.
#[cfg(not(feature = "postgres"))]
#[inline]
#[must_use]
pub fn pq(sql: &str) -> &str {
    sql
}

/// Rewrite SQLite `?N` placeholders to Postgres `$N`. See the SQLite twin for
/// the contract.
#[cfg(feature = "postgres")]
#[must_use]
pub fn pq(sql: &str) -> String {
    let bytes = sql.as_bytes();
    let mut out = String::with_capacity(sql.len() + 8);
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'?' && i + 1 < bytes.len() && bytes[i + 1].is_ascii_digit() {
            // A `?N` positional placeholder: emit `$N`, copying the digit run.
            out.push('$');
            i += 1;
            while i < bytes.len() && bytes[i].is_ascii_digit() {
                out.push(bytes[i] as char);
                i += 1;
            }
        } else {
            // Copy this UTF-8 sequence verbatim. `?` is ASCII, so byte-indexing
            // only splits the string at a char boundary here; push the whole
            // char to stay UTF-8 correct.
            let ch = sql[i..].chars().next().expect("valid char boundary");
            out.push(ch);
            i += ch.len_utf8();
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::pq;

    #[test]
    fn identity_or_translation() {
        let sql = "SELECT a FROM t WHERE b = ?1 AND c = ?2 ORDER BY d";
        #[cfg(not(feature = "postgres"))]
        assert_eq!(pq(sql), sql);
        #[cfg(feature = "postgres")]
        assert_eq!(
            pq(sql),
            "SELECT a FROM t WHERE b = $1 AND c = $2 ORDER BY d"
        );
    }

    #[cfg(feature = "postgres")]
    #[test]
    fn leaves_lone_question_marks_and_text() {
        // A `?` not followed by a digit (e.g. inside a string literal) is left
        // alone, and multi-digit indices are copied whole.
        assert_eq!(pq("a ? b ?10 c"), "a ? b $10 c");
    }
}

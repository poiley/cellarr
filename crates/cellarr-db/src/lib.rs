//! cellarr-db — the persistence layer.
//!
//! The **only** crate that talks to a database. It owns the schema (sqlx
//! migrations), the single writer-actor, the cache table, FTS, and the concrete
//! repository types that implement `cellarr-core`'s repository traits. Nothing
//! else writes SQL.
//!
//! # Engine
//!
//! The engine is selected **at compile time** ([`dialect`]): SQLite by default
//! (WAL mode, a nonzero `busy_timeout`, all writes funnelled through one task
//! using `BEGIN IMMEDIATE` so the single-writer reality is explicit rather than
//! fought — docs/08-database.md), or **Postgres** under the `postgres` cargo
//! feature (a database server reached over TCP, for deployments whose storage
//! makes a file-based engine slow — a SQLite file on a network mount pays a
//! round-trip per page). Each path uses sqlx's native driver; the repository
//! code is written once against the [`dialect`] aliases and reads as one
//! backend-agnostic implementation.
//!
//! # Queries
//!
//! This first pass uses the sqlx **runtime** query API (`query`/`query_as`),
//! not the compile-time `query!` macros, so the crate builds without a live DB.
//! Committing offline `.sqlx` metadata and switching to compile-time-checked
//! queries is a planned follow-up.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

mod convert;
mod db;
mod dialect;
mod error;
mod repos;
mod writer;

pub use db::Database;
pub use error::{DbError, Result};
pub use repos::{
    AuthRepo, AuthSession, BlocklistRepo, CacheRepo, ConfigRepo, ContentRepo, DecisionLogRepo,
    GrabRepo, HistoryRepo, ImportListRepo, ManagedConfigRepo, ManagedEntity, MediaFileRepo,
    PendingRelease, PendingReleaseRepo, ProfileRepo, TagRepo,
};
pub use writer::WriterHandle;

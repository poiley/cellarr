//! cellarr-db — the persistence layer.
//!
//! The **only** crate that talks to a database. It owns the schema (sqlx
//! migrations), the single writer-actor, the cache table, FTS, and the concrete
//! repository types that implement `cellarr-core`'s repository traits. Nothing
//! else writes SQL.
//!
//! # Engine
//!
//! Default and only wired engine in this pass is **SQLite** in WAL mode with a
//! nonzero `busy_timeout`; all writes funnel through one task using
//! `BEGIN IMMEDIATE` so the single-writer reality is explicit rather than
//! fought (docs/08-database.md). Postgres lives behind the `postgres` cargo
//! feature and is **deferred** (stub) — the repository interface is identical,
//! so callers are engine-agnostic when it lands.
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
mod error;
mod repos;
mod writer;

pub use db::Database;
pub use error::{DbError, Result};
pub use repos::{
    BlocklistRepo, CacheRepo, ConfigRepo, ContentRepo, DecisionLogRepo, GrabRepo, HistoryRepo,
    ImportListRepo, MediaFileRepo, ProfileRepo,
};
pub use writer::WriterHandle;

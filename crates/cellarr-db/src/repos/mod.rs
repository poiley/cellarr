//! Concrete repository types implementing `cellarr-core`'s repository traits.
//!
//! Each repo holds a read pool and a clone of the [`crate::writer::WriterHandle`]:
//! reads run on the pool, writes go through the single writer-actor. Nothing
//! outside this module writes SQL.
//!
//! This first pass uses the **runtime** query API (`sqlx::query`/`query_as`),
//! not the compile-time `query!` macros, so the crate builds without a live DB
//! or committed offline `.sqlx` metadata. Switching to compile-time-checked
//! queries + a committed `.sqlx` directory is a planned follow-up.

mod cache;
mod config;
mod content;
mod decision_log;
mod grab;
mod history;
mod profile;

pub use cache::CacheRepo;
pub use config::ConfigRepo;
pub use content::{ContentNode, ContentRepo};
pub use decision_log::DecisionLogRepo;
pub use grab::{GrabRepo, GrabStatus};
pub use history::HistoryRepo;
pub use profile::ProfileRepo;

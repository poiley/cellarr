//! The typed error surface for managed-config loading and reconciliation.
//!
//! Every failure mode the task calls out — malformed YAML, an unknown field, an
//! unresolved required secret, a broken cross-reference, an unsupported schema
//! version, a repo/IO failure during apply — maps to a distinct variant carrying
//! a clear, operator-facing message. The CLI prints these; boot fails loudly on
//! any of them (a half-applied or stale config must never serve).

use std::path::PathBuf;

use thiserror::Error;

/// An error from loading, validating, planning, or applying a managed config.
#[derive(Debug, Error)]
pub enum ManagedError {
    /// The configured file could not be read from disk.
    #[error("reading managed config file {path}: {source}")]
    Read {
        /// The path that failed to read.
        path: PathBuf,
        /// The underlying IO error.
        source: std::io::Error,
    },

    /// The file is not valid YAML, or a value did not fit the schema (including an
    /// unknown field — the schema is `deny_unknown_fields`).
    #[error("parsing managed config: {0}")]
    Parse(String),

    /// A `${VAR}` reference (with no default) named a variable that is not set in
    /// the process environment. The message names the variable so the operator
    /// knows exactly which secret to provide.
    #[error(
        "unresolved required secret: environment variable `{var}` is referenced \
         by the managed config but is not set (and has no `:-default`)"
    )]
    UnresolvedSecret {
        /// The missing environment variable name.
        var: String,
    },

    /// A malformed interpolation reference (an unterminated `${`, an empty `${}`,
    /// or an invalid variable name).
    #[error("invalid secret interpolation: {0}")]
    Interpolation(String),

    /// The file declared an `apiVersion` this build does not understand.
    #[error("unsupported managed config apiVersion `{found}` (this build supports `{supported}`)")]
    UnsupportedApiVersion {
        /// The version the file declared.
        found: String,
        /// The version this build supports.
        supported: &'static str,
    },

    /// A cross-reference, duplicate name, or other semantic check failed. The
    /// message describes the broken reference precisely (which item references
    /// what missing target).
    #[error("invalid managed config: {0}")]
    Validation(String),

    /// A repository call failed while applying the plan to the database.
    #[error("applying managed config to the database: {0}")]
    Apply(#[from] cellarr_db::DbError),
}

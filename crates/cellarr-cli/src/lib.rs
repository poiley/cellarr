//! cellarr — daemon wiring, exposed as a library so the binary and the
//! integration tests share one assembly path.
//!
//! This crate is the **only** place wiring happens (`docs/specs/cellarr-cli.md`):
//! the layered [`config`], the [`boot`] sequence (open DB → build registries →
//! start scheduler → start API), and the runtime [`registry`] construction. The
//! binary ([`main`](../main.rs)) is a thin clap front end over these.

#![forbid(unsafe_code)]

pub mod boot;
pub mod clients;
pub mod config;
pub mod managed;
pub mod metadata;
pub mod pipeline;
pub mod registry;
pub mod resolver;

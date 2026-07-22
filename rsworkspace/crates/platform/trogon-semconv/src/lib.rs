#![cfg_attr(test, allow(clippy::expect_used, clippy::panic, clippy::unwrap_used))]

//! Generated OpenTelemetry semantic conventions for TrogonAi.
//!
//! The source of truth is the Weaver registry under `otel/semconv/` at the repository
//! root. The contents of `gen/` are generated; regenerate with
//! `mise run semconv:generate` and do not edit them by hand.

#[allow(clippy::all)]
mod r#gen;

pub use r#gen::{attribute, metric, span};

//! Policy layer for the A2A gateway.
//!
//! Subsequent extraction slices (g8 → g11) layer the individual tiers on
//! top of this scaffold:
//! - **Tier 1 declarative** — bundle-driven decision rules
//! - **Tier 1 SpiceDB** — relational-graph authorization
//! - **Tier 2 CEL** — per-skill CEL evaluation
//! - **Tier 3 redaction** — Wasm-driven content redaction
//!
//! This slice ships only the shared [`error`] surface so each tier PR can
//! land independently without re-exporting a moving error enum.

pub mod error;

pub use error::{PolicyError, Tier2EvalError};

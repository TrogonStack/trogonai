//! Policy layer for the A2A gateway.
//!
//! Subsequent extraction slices layer the remaining tiers on top of this
//! scaffold:
//! - **Tier 1 declarative** — bundle-driven decision rules (shipped here)
//! - **Tier 1 SpiceDB** — relational-graph authorization
//! - **Tier 2 CEL** — per-skill CEL evaluation
//! - **Tier 3 redaction** — Wasm-driven content redaction
//!
//! Each tier folds its own error variant into [`error::PolicyError`] so
//! the runtime keeps a single audit surface across tiers.

pub mod error;
pub mod tier1_declarative;

pub use error::{PolicyError, Tier2EvalError};
pub use tier1_declarative::{
    ENV_TIER1_BUNDLE_DIR, ENV_TIER1_DECLARATIVE_ENABLED, FixedTier1Clock, GatewayTier1DeclarativeLayer,
    NoopTier1DeclarativeGate, RealTier1DeclarativeGate, SystemTier1Clock, TIER1_BUNDLE_EXTENSION, Tier1Clock,
    Tier1DeclarativeBuildError, Tier1DeclarativeBundle, Tier1DeclarativeConfig, Tier1DeclarativeContext,
    Tier1DeclarativeDecision, Tier1DeclarativeEffect, Tier1DeclarativeGate, Tier1DeclarativeLoadError,
    Tier1DeclarativeMatch, Tier1DeclarativeRule, Tier1DeclarativeRuleId, Tier1DeclarativeSchemaError,
    Tier1ResourceKind, tier1_declarative_audit_rule_fired,
};

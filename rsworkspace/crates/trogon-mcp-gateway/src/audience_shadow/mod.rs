//! Shadow-mode inbound mesh `aud` validation (audit-only until enforce).
//!
//! **Future gateway hook (not wired in this PR):** after `JwtValidator::resolve` succeeds in
//! `jwt.rs` and before CEL/policy evaluation in `gateway.rs`, call
//! `AudienceShadowChecker::check` with `ClaimAud` parsed from the verified JWT payload,
//! `tenant` + gateway `server_id` (or backend target on egress), `AudienceShadowMode` from
//! `MCP_GATEWAY_AGENT_IDENTITY`, and an `AudienceAuditSink` adapter that publishes
//! `mcp.audit.gateway.aud_mismatch` via `audit.rs`. On `AudienceShadowOutcome::EnforceMismatch`,
//! map to JSON-RPC `-32109` (`rpc_codes::AUDIENCE_MISMATCH`); shadow mismatch must not reject.

mod aud_format;
mod audit_sink;
mod checker;
mod errors;
mod mode;

pub use aud_format::compute_expected_aud;
pub use audit_sink::{
    AudMismatchEnvelope, AudienceAuditSink, RecordingAuditSink, AUD_MISMATCH_AUDIT_SUBJECT,
};
pub use checker::{AudienceCheckContext, AudienceShadowChecker, ClaimAud};
pub use errors::AudienceShadowError;
pub use mode::AudienceShadowMode;

/// Result of comparing inbound `aud` to the expected mesh audience for this hop.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AudienceShadowOutcome {
    /// `AudienceShadowMode::Off` — comparison skipped.
    NoOp,
    /// Claim includes the expected audience URI.
    Match,
    /// Shadow mode: mismatch audited; request must continue.
    ShadowMismatch,
    /// Enforce mode: mismatch audited; caller maps to `-32109`.
    EnforceMismatch,
}

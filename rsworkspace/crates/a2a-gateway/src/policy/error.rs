//! Policy-layer error surface shared across every policy tier.
//!
//! The tier modules each report their own structured failures (redaction
//! rewrite errors, CEL eval failures, SpiceDB lookup failures, etc.), and
//! the runtime needs to bubble those up through a single typed enum so the
//! audit subject can record `policy_error.kind` without losing the source
//! chain. `thiserror` carries that source through `#[from]` / `#[source]`.

use a2a_redaction::RedactionError;

/// Errors surfaced from the policy layer to the runtime.
///
/// New tiers (Tier1 declarative, Tier1 SpiceDB) get folded in as their
/// extraction PRs land; this slice scaffolds the enum with the two tiers
/// already extracted (`a2a-redaction` for Tier3 rewrites and the Tier2 CEL
/// evaluator). Each variant chains the underlying source so structured
/// logging surfaces the root cause instead of a flattened string.
#[derive(Debug, thiserror::Error)]
pub enum PolicyError {
    #[error("redaction tier failed")]
    Redaction(#[from] RedactionError),
    #[error("tier2 evaluation failed")]
    Tier2(#[from] Tier2EvalError),
}

/// Failure surface for the Tier2 CEL evaluator. The CEL evaluator's own
/// error type carries spans + interpreter state we don't want crossing the
/// crate boundary, so this opaque newtype keeps the audit log message
/// stable while still letting the source chain flow through `Display`.
#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct Tier2EvalError(Box<str>);

impl Tier2EvalError {
    #[must_use]
    pub fn new(message: impl Into<Box<str>>) -> Self {
        Self(message.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[cfg(test)]
mod tests;

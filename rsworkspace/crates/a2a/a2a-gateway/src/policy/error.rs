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

/// Failure surface for the Tier2 CEL evaluator. The CEL interpreter's own
/// error type carries spans + interpreter state we don't want crossing the
/// crate boundary, so each variant collapses its source into the message
/// while letting callers route on the variant tag (execution vs. binding
/// vs. wrong-result-type) instead of pattern-matching on `Display`.
#[derive(Debug, thiserror::Error)]
pub enum Tier2EvalError {
    /// `Program::execute` returned an interpreter error. The collapsed
    /// message preserves the interpreter's own description without
    /// dragging the cel-interpreter types across the crate boundary.
    #[error("CEL execution failed: {message}")]
    Execution { message: Box<str> },
    /// The rule returned a value other than `bool`. CEL rules MUST
    /// evaluate to a boolean — anything else is a misconfigured rule
    /// that must be denied closed rather than silently coerced.
    #[error("CEL rule must return bool, got {value_type}")]
    NonBoolResult { value_type: Box<str> },
    /// One of the variable bindings (request, caller, agent, task,
    /// headers) failed to serialize into the CEL context.
    #[error("CEL binding `{binding}` failed: {message}")]
    Binding { binding: Box<str>, message: Box<str> },
}

impl Tier2EvalError {
    /// Convenience for the execution-failed path.
    #[must_use]
    pub fn execution(message: impl Into<Box<str>>) -> Self {
        Self::Execution {
            message: message.into(),
        }
    }

    /// Convenience for the non-bool-result path.
    #[must_use]
    pub fn non_bool_result(value_type: impl Into<Box<str>>) -> Self {
        Self::NonBoolResult {
            value_type: value_type.into(),
        }
    }

    /// Convenience for a binding-stage failure.
    #[must_use]
    pub fn binding(binding: impl Into<Box<str>>, message: impl Into<Box<str>>) -> Self {
        Self::Binding {
            binding: binding.into(),
            message: message.into(),
        }
    }
}

#[cfg(test)]
mod tests;

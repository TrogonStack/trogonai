use crate::failure_reason::FailureReason;

/// A must-audit record could not be durably written, so the action that needed it
/// is refused.
///
/// This is an operational fault and never a security denial: the surface maps it
/// to its own unavailable answer (`AUDIT_UNAVAILABLE` with 503 and `Retry-After`
/// on HTTP) and keeps it distinct from "you may not".
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("audit is unavailable: {reason}")]
pub struct AuditUnavailableError {
    reason: FailureReason,
}

impl AuditUnavailableError {
    pub(crate) fn new(reason: FailureReason) -> Self {
        Self { reason }
    }

    pub fn reason(&self) -> FailureReason {
        self.reason
    }
}

#[cfg(test)]
mod tests;

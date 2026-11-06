use trogon_audit::{AuditNameError, AuditUnavailableError, ReasonCodeError};

/// Why a decision did not reach the audit stream.
///
/// Only [`AuditError::Unavailable`] can happen to a well-formed decision. The
/// other variants mean a fact of the decision is not a name an audit record may
/// carry; no destination or agent the proxy admits is one, so they guard the
/// boundary rather than describe a case the proxy expects to meet.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum AuditError {
    #[error("audit write capacity is exhausted")]
    Overloaded,
    #[error("audit could not take the record")]
    Unavailable(#[from] AuditUnavailableError),
    #[error("a fact of the decision is not a name an audit record may carry")]
    Name(#[from] AuditNameError),
    #[error("a refusal has no usable audit reason code")]
    Reason(#[from] ReasonCodeError),
    #[error("policy provenance does not match the decision")]
    PolicyEvidence,
    #[error("authenticated workload evidence does not match the decision")]
    WorkloadEvidence(#[from] trogon_audit::WorkloadPeerScopeError),
}

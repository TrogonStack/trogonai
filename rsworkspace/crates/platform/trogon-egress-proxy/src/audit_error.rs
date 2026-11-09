use trogon_audit::{AuditNameError, AuditUnavailableError, ReasonCodeError};

/// Why a decision did not reach the audit stream.
///
/// A well-formed decision can fail when storage is unavailable or every audit
/// worker is occupied. Other variants guard the names and provenance a record
/// is allowed to carry.
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

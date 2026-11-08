use trogon_audit::{AuditNameError, AuditUnavailableError, ReasonCodeError};

/// Why a decision did not reach the audit stream.
///
/// Only [`AuditError::Unavailable`] can happen to a well-formed decision. The
/// other two mean a fact of the decision could not be spelled as an audit record
/// field. Names are digested when they do not fit, so those two guard the
/// boundary rather than describe a case the gateway expects to meet. An allow
/// that meets any of them is refused all the same, because an allow nobody can
/// record must not be forwarded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum AuditError {
    #[error("audit could not take the record")]
    Unavailable(#[from] AuditUnavailableError),
    #[error("a fact of the decision is not a name an audit record may carry")]
    Name(#[from] AuditNameError),
    #[error("a refusal has no usable audit reason code")]
    Reason(#[from] ReasonCodeError),
}

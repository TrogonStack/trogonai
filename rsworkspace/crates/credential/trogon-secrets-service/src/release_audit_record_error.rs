use trogon_audit::{AuditNameError, ReasonCodeError};
use trogon_credential_aggregate::CredentialEventSubjectResolverError;

/// A release decision the audit record cannot carry. The record is never
/// written, so a release that cannot be recorded is refused.
#[derive(Debug, thiserror::Error)]
pub enum ReleaseAuditRecordError {
    #[error("{field} is not an audit name: {source}")]
    Name {
        field: &'static str,
        #[source]
        source: AuditNameError,
    },
    #[error("the credential's project is not a subject token: {0}")]
    Project(#[source] CredentialEventSubjectResolverError),
    #[error("the denial reason is not a reason code: {0}")]
    Reason(#[source] ReasonCodeError),
}

#[cfg(test)]
mod tests;

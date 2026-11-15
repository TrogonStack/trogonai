use trogon_credential_domain::RuntimeServiceIdError;
use trogon_identity_types::SubjectPrincipalError;

use super::TenantAccountError;

/// Why a subject names no caller the release endpoint serves.
///
/// It goes to the logs only. The caller is answered with the same denial
/// whatever the reason, so it cannot tell which part of its subject was
/// refused.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum ReleaseCallerError {
    #[error("subject is not the release endpoint")]
    OutsideEndpoint,
    #[error("subject tenant token is not a tenant account")]
    Tenant(#[source] TenantAccountError),
    #[error("subject principal position names no principal")]
    Principal(#[source] SubjectPrincipalError),
    #[error("subject principal is not a service")]
    NotAService,
    #[error("subject service id is not a runtime service id")]
    RuntimeService(#[source] RuntimeServiceIdError),
}

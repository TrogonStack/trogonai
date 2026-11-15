use trogon_credential_domain::{
    AuditCorrelationIdError, CredentialIdError, CredentialOwnerIdError, InjectionLocationError, ReleaseHostError,
};

/// Why a release request payload is not a call the release core can judge.
///
/// The exchange contract answers it on the ADR#0016 error channel with
/// `INVALID_ARGUMENT`, never with a typed failure. It concerns only what the
/// caller wrote, so naming the field tells the caller nothing about any tenant,
/// credential, or policy.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum ReleaseCallError {
    #[error("release request names no credential")]
    MissingCredential,
    #[error("release request credential id is invalid")]
    CredentialId(#[source] CredentialIdError),
    #[error("release request credential owner id is invalid")]
    OwnerId(#[source] CredentialOwnerIdError),
    #[error("release request names no destination host")]
    MissingDestination,
    #[error("release request destination host is a pattern, not an exact host")]
    DestinationNotExact,
    #[error("release request destination host is invalid")]
    DestinationHost(#[source] ReleaseHostError),
    #[error("release request destination port {0} is outside 1 to 65535")]
    DestinationPort(u32),
    #[error("release request names no injection location")]
    MissingLocation,
    #[error("release request injection location is invalid")]
    Location(#[source] InjectionLocationError),
    #[error("release request names no correlation id")]
    MissingCorrelationId,
    #[error("release request correlation id is invalid")]
    CorrelationId(#[source] AuditCorrelationIdError),
    #[error("release request names no requester xkey")]
    MissingRequesterXkey,
    #[error("release request requester xkey is not an X followed by 55 base32 characters")]
    RequesterXkeyPattern,
}

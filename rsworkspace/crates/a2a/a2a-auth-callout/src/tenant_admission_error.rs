use crate::admission_keys::AccountPublicKeyError;

#[derive(Debug, thiserror::Error)]
pub enum TenantAdmissionError {
    #[error("authenticated caller has no tenant membership")]
    NotMember,
    #[error("verified credential audience differs from requested tenant")]
    AudienceMismatch,
    #[error("tenant membership registry could not be read")]
    Read(#[source] std::io::Error),
    #[error("tenant membership registry is not a regular file")]
    NotRegularFile,
    #[error("tenant membership registry exceeds its size limit")]
    TooLarge,
    #[error("tenant membership registry exceeds its membership limit")]
    TooManyMemberships,
    #[error("tenant membership registry JSON is invalid")]
    Json(#[source] serde_json::Error),
    #[error("tenant membership registry version is unsupported")]
    UnsupportedVersion,
    #[error("tenant membership account is invalid")]
    Account(#[source] AccountPublicKeyError),
    #[error("tenant membership OIDC issuer is invalid")]
    OidcIssuer(#[source] crate::credentials::oidc::OidcIssuerUrlError),
    #[error("tenant membership principal is invalid")]
    Principal(#[source] trogon_identity_types::SubjectPrincipalError),
    #[error("tenant membership principal identifier is invalid")]
    PrincipalId(#[source] trogon_identity_types::PrincipalIdError),
    #[error("verified tenant membership claims are invalid")]
    Claims(#[source] crate::jwt::JwtError),
    #[error("tenant membership registry contains a duplicate binding")]
    Duplicate,
    #[error("tenant membership authority is busy")]
    Busy,
    #[error("tenant membership authority did not respond in time")]
    Timeout,
    #[error("tenant membership worker failed")]
    Worker(#[source] tokio::task::JoinError),
}

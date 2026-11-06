#[derive(Debug, thiserror::Error)]
pub enum RegistrationError {
    #[error("protected registration publication is unavailable")]
    NotImplemented,
    #[error("registration scope name is invalid")]
    ScopeName,
    #[error("registration project is not a supported subject token")]
    Project(#[source] trogon_nats::SubjectTokenViolationError),
    #[error("registration trust domain is invalid")]
    TrustDomain(#[source] spiffe::SpiffeIdError),
    #[error("registration trust domain is not an exact canonical domain")]
    NoncanonicalDomain,
    #[error("registration approval key has insufficient order")]
    WeakApprovalKey,
    #[error("registration approval signature is invalid")]
    Signature(#[source] ed25519_dalek::SignatureError),
    #[error("registration payload is malformed")]
    Decode(#[source] buffa::DecodeError),
    #[error("registration payload is not the supported canonical protobuf")]
    NoncanonicalPayload,
    #[error("registration resource bounds are invalid")]
    Bounds,
    #[error("registration identity binding is invalid")]
    Binding(#[source] trogon_workload_identity::RegistrationError),
    #[error("registration scope or independently configured recovery fence differs")]
    Scope,
    #[error("registration revision regressed, replayed or equivocated")]
    Revision,
    #[error("registration approval has no current finite lease")]
    Lease,
    #[error("registration agent identity is not a canonical Ed25519 did:key")]
    ApplicationIdentity,
    #[error("registration agent identity encoding is invalid")]
    Base58(#[source] bs58::decode::Error),
    #[error("registration agent public key is invalid")]
    ApplicationKey(#[source] ed25519_dalek::SignatureError),
}

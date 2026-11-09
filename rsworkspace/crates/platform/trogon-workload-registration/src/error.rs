#[derive(Debug, thiserror::Error)]
pub enum RegistrationError {
    #[error("registration requires authenticated verified TLS")]
    Transport,
    #[error("registration connection failed")]
    Connection(#[source] trogon_nats::ConnectError),
    #[error("registration stream lookup failed")]
    Stream(#[source] async_nats::jetstream::context::GetStreamError),
    #[error("registration stream provisioning failed")]
    Provision(#[source] async_nats::jetstream::context::CreateStreamError),
    #[error("registration stream inspection failed")]
    Inspect(#[source] async_nats::jetstream::stream::InfoError),
    #[error("registration stored message lookup failed")]
    Read(#[source] async_nats::jetstream::stream::RawMessageError),
    #[error("registration compare-and-publish failed")]
    Publish(#[source] async_nats::jetstream::context::PublishError),
    #[error("registration stream does not enforce the protected profile")]
    StreamProfile,
    #[error("registration authenticated connection continuity changed")]
    ConnectionChanged,
    #[error("registration stream incarnation or acknowledged head changed")]
    HeadChanged,
    #[error("registration ownership cannot be reclaimed in this recovery fence")]
    AlreadyClaimed,
    #[error("registration writer is permanently poisoned")]
    Poisoned,
    #[error("registration set changes require a signed whole-scope withdrawal first")]
    WithdrawalRequired,
    #[error("emergency withdrawal requires an empty binding or root set")]
    NotWithdrawal,
    #[error("registration operation exceeded its bounded approval window")]
    Timeout,
    #[error("registration audit name is invalid")]
    AuditName(#[source] trogon_audit::AuditNameError),
    #[error("registration audit was not acknowledged")]
    Audit(#[source] trogon_audit::AuditUnavailableError),
    #[error("withdrawal is live at sequence {live_sequence}, but bookkeeping or audit is incomplete")]
    WithdrawalIncomplete {
        live_sequence: std::num::NonZeroU64,
        control: Option<Box<RegistrationError>>,
        audit: Option<Box<RegistrationError>>,
    },
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

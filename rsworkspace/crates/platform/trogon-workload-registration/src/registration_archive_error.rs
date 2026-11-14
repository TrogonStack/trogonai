#[derive(thiserror::Error)]
pub enum RegistrationArchiveError {
    #[error("archive capacity is not a supported finite deployment limit")]
    Capacity,
    #[error("archive public artifact bytes exceed their closed kind bounds")]
    ArtifactBounds,
    #[error("archive subject is invalid")]
    Subject(#[source] trogon_nats::SubjectTokenViolationError),
    #[error("archive requires authenticated verified TLS")]
    Transport,
    #[error("archive connection failed")]
    Connection(#[source] trogon_nats::ConnectError),
    #[error("archive original connection continuity changed")]
    Source(#[source] crate::RegistrationError),
    #[error("archive native stream lookup failed")]
    Stream(#[source] async_nats::jetstream::context::GetStreamError),
    #[error("archive native stream inspection failed")]
    Inspect(#[source] async_nats::jetstream::stream::InfoError),
    #[error("archive immutable stream profile differs")]
    Profile,
    #[error("archive original stream creation changed")]
    Incarnation,
    #[error("archive pre-write operation deadline expired")]
    Deadline(#[source] tokio::time::error::Elapsed),
    #[error("archive native create outcome may have written bytes")]
    WriteUncertain(#[source] trogon_nats::jetstream::NativeKvCasError),
    #[error("archive possible-write operation deadline expired")]
    WriteDeadline(#[source] tokio::time::error::Elapsed),
    #[error("archive native create was acknowledged but source checks failed")]
    AcknowledgedButUnusable {
        native_revision: std::num::NonZeroU64,
        #[source]
        source: Box<RegistrationArchiveError>,
    },
    #[error("archive record is missing in the observed native source")]
    Missing,
    #[error("archive original bytes do not match their digest address")]
    DigestMismatch,
    #[error("archive record metadata or identity is inconsistent")]
    Integrity,
    #[error("archive record is not an original Put")]
    Operation,
    #[error("archive explicit deployment capacity has been reached")]
    CapacityReached,
    #[error("archive original native frontier changed before conditional write")]
    Conflict,
    #[error("archive original finite operation deadline expired")]
    DeadlineExpired,
    #[error("archive native bucket lookup failed")]
    Bucket(#[source] async_nats::jetstream::context::KeyValueError),
    #[error("archive native binding is invalid")]
    NativeBinding(#[source] trogon_nats::jetstream::NativeKvCasError),
    #[error("archive strict native record read failed")]
    NativeRead(#[source] trogon_nats::jetstream::NativeStreamReadError),
    #[error("archive original KV operation metadata is malformed")]
    NativeOperation(#[source] trogon_nats::jetstream::NativeKvReadError),
    #[error("neutral archive create is not implemented")]
    Unsupported,
}
impl std::fmt::Debug for RegistrationArchiveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self}")
    }
}

#[derive(Debug, thiserror::Error)]
pub enum PeerMcpError {
    #[error("Peer detach was not confirmed; lease expiry remains the fallback.")]
    EnrollmentUncertain,
    #[error("Peer connector is detached; start a new connector runtime.")]
    Detached,
    #[error("Peer detach was not confirmed; lease expiry remains the fallback.")]
    Detach(#[source] Box<dyn std::error::Error + Send + Sync>),
    #[error("Peer detach was not confirmed; lease expiry remains the fallback.")]
    DetachDeadline,
    #[error("Peer hook deadline elapsed; retry the same prompt identity. Remote outcome is uncertain.")]
    PromptDeadline,
    #[error("Peer connector has not enrolled a native session.")]
    Unbound,
    #[error("This connector belongs to another native session; reconnect it for this session.")]
    NativeSessionChanged,
    #[error("Peer tool arguments are invalid.")]
    InvalidArguments(#[source] serde_json::Error),
    #[error("Peer service returned an invalid delivery assignment.")]
    InvalidBatch(#[source] trogon_peer_context::PeerContextError),
    #[error("Peer connector configuration is invalid.")]
    InvalidConfig(#[source] trogon_peer::PeerValueError),
    #[error("Peer tool arguments are invalid.")]
    InvalidHostIdentity(#[source] trogon_peer::PeerValueError),
    #[error("Peer tool arguments are invalid.")]
    InvalidModelInput(#[source] trogon_peer::PeerValueError),
    #[error("Peer service returned invalid operation metadata.")]
    InvalidMetadata,
    #[error("Peer service returned invalid operation metadata.")]
    MetadataEncoding(#[source] serde_json::Error),
    #[error("Peer service returned an invalid enrollment.")]
    InvalidEnrollment,
    #[error("Peer service returned an invalid enrollment.")]
    InvalidEnrollmentIdentity(#[source] trogon_peer::PeerValueError),
    #[error("Peer operation is not available in this adapter layer.")]
    UnsupportedTool,
    #[error("Peer batch retry content is unavailable; its original preparation record is retained.")]
    RetryContentUnavailable,
    #[error("Peer hook output could not be encoded.")]
    Encode(#[source] serde_json::Error),
    #[error("Peer service is unavailable; pending delivery remains queued.")]
    Enroll(#[source] Box<dyn std::error::Error + Send + Sync>),
    #[error("Peer service is unavailable; pending delivery remains queued.")]
    Prepare(#[source] Box<dyn std::error::Error + Send + Sync>),
    #[error("Peer application acknowledgment was not confirmed; retry the same batch.")]
    Acknowledge(#[source] Box<dyn std::error::Error + Send + Sync>),
    #[error("Peer metadata is unavailable.")]
    Roster(#[source] Box<dyn std::error::Error + Send + Sync>),
    #[error("Peer send outcome was not confirmed; retry with the same idempotency key.")]
    Send(#[source] Box<dyn std::error::Error + Send + Sync>),
    #[error("Peer metadata is unavailable.")]
    Status(#[source] Box<dyn std::error::Error + Send + Sync>),
}

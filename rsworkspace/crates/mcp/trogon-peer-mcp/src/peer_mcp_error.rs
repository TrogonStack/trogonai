#[derive(Debug, thiserror::Error)]
pub enum PeerMcpError {
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
}

impl PeerMcpError {
    pub const fn code(&self) -> crate::PeerMcpErrorCode {
        use crate::PeerMcpErrorCode;

        match self {
            Self::PromptDeadline => PeerMcpErrorCode::PromptDeadline,
            Self::Unbound => PeerMcpErrorCode::Unbound,
            Self::NativeSessionChanged => PeerMcpErrorCode::NativeSessionChanged,
            Self::InvalidArguments(_) => PeerMcpErrorCode::InvalidArguments,
            Self::InvalidBatch(_) => PeerMcpErrorCode::InvalidBatch,
            Self::InvalidConfig(_) => PeerMcpErrorCode::InvalidConfig,
            Self::InvalidHostIdentity(_) => PeerMcpErrorCode::InvalidHostIdentity,
            Self::InvalidEnrollment => PeerMcpErrorCode::InvalidEnrollment,
            Self::InvalidEnrollmentIdentity(_) => PeerMcpErrorCode::InvalidEnrollmentIdentity,
            Self::UnsupportedTool => PeerMcpErrorCode::UnsupportedTool,
            Self::RetryContentUnavailable => PeerMcpErrorCode::RetryContentUnavailable,
            Self::Encode(_) => PeerMcpErrorCode::Encode,
            Self::Enroll(_) => PeerMcpErrorCode::Enroll,
            Self::Prepare(_) => PeerMcpErrorCode::Prepare,
        }
    }
}

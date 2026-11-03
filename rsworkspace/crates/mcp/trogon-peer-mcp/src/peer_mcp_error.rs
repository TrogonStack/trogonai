#[derive(Debug, thiserror::Error)]
pub enum PeerMcpError {
    #[error("peer connector has not enrolled a native session")]
    Unbound,
    #[error("peer connector cannot switch native sessions")]
    NativeSessionChanged,
    #[error("peer tool arguments are invalid: {0}")]
    InvalidArguments(#[source] serde_json::Error),
    #[error("peer backend returned an invalid delivery batch")]
    InvalidBatch(#[source] trogon_peer_context::PeerContextError),
    #[error("peer connector configuration is invalid")]
    InvalidConfig(#[source] trogon_peer::PeerValueError),
    #[error("native hook identity is invalid")]
    InvalidHostIdentity(#[source] trogon_peer::PeerValueError),
    #[error("peer model operation input is invalid")]
    InvalidModelInput(#[source] trogon_peer::PeerValueError),
    #[error("peer backend returned invalid operation metadata")]
    InvalidMetadata,
    #[error("peer backend returned an invalid enrollment")]
    InvalidEnrollment,
    #[error("peer backend returned an invalid enrollment identity")]
    InvalidEnrollmentIdentity(#[source] trogon_peer::PeerValueError),
    #[error("peer operation is not available in this adapter layer")]
    UnsupportedTool,
    #[error("prepared batch retry content is unavailable")]
    RetryContentUnavailable,
    #[error("peer hook output could not be encoded")]
    Encode(#[source] serde_json::Error),
    #[error("peer enrollment failed")]
    Enroll(#[source] Box<dyn std::error::Error + Send + Sync>),
    #[error("peer batch preparation failed")]
    Prepare(#[source] Box<dyn std::error::Error + Send + Sync>),
    #[error("peer batch acknowledgment failed")]
    Acknowledge(#[source] Box<dyn std::error::Error + Send + Sync>),
}

impl PeerMcpError {
    pub(crate) fn public_message(&self) -> &'static str {
        match self {
            Self::Unbound => "Peer connector has not enrolled a native session.",
            Self::NativeSessionChanged => {
                "This connector belongs to another native session; reconnect it for this session."
            }
            Self::InvalidArguments(_) | Self::InvalidHostIdentity(_) | Self::InvalidModelInput(_) => {
                "Peer tool arguments are invalid."
            }
            Self::InvalidMetadata => "Peer service returned invalid operation metadata.",
            Self::InvalidBatch(_) => "Peer service returned an invalid delivery assignment.",
            Self::InvalidConfig(_) => "Peer connector configuration is invalid.",
            Self::InvalidEnrollment | Self::InvalidEnrollmentIdentity(_) => {
                "Peer service returned an invalid enrollment."
            }
            Self::UnsupportedTool => "Peer operation is not available in this adapter layer.",
            Self::RetryContentUnavailable => {
                "Peer batch retry content is unavailable; its original preparation record is retained."
            }
            Self::Enroll(_) | Self::Prepare(_) | Self::Acknowledge(_) => {
                "Peer service is unavailable; pending delivery remains queued."
            }
            Self::Encode(_) => "Peer hook output could not be encoded.",
        }
    }
}

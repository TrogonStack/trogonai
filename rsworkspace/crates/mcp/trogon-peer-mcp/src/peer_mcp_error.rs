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
    InvalidConfig,
    #[error("peer enrollment failed")]
    Enroll(#[source] Box<dyn std::error::Error + Send + Sync>),
    #[error("peer batch preparation failed")]
    Prepare(#[source] Box<dyn std::error::Error + Send + Sync>),
}

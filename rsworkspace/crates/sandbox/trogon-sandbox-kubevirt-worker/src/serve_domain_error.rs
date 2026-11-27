use trogon_sandbox_protocol::NatsTransferError;

/// Why a get_domain request could not be answered.
#[derive(Debug, thiserror::Error)]
pub enum ServeDomainError {
    /// The request could not be received or the reply could not be sent.
    #[error(transparent)]
    Transfer(#[from] NatsTransferError),
    /// The request or the reply did not parse or serialize.
    #[error("invalid get_domain payload: {0}")]
    Json(#[from] serde_json::Error),
}

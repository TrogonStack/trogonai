/// Why a NATS connection could not be opened.
#[derive(Debug, thiserror::Error)]
pub enum NatsConnectError {
    /// The server URL did not parse.
    #[error("invalid NATS URL: {0}")]
    InvalidUrl(#[from] url::ParseError),
    /// The URL names a user but no password.
    #[error("NATS URL has a user but no password")]
    MissingPassword(#[from] trogon_std::secret_string::EmptySecretError),
    /// The connection attempt failed.
    #[error(transparent)]
    Connect(#[from] trogon_nats::ConnectError),
}

use trogon_std::EmptySecretError;

#[derive(Debug, thiserror::Error)]
pub enum OpenBaoSecretStoreConfigError {
    #[error("invalid OpenBao address: {0}")]
    InvalidAddress(#[source] url::ParseError),
    #[error("OpenBao address must use https unless plaintext transport is allowed")]
    PlaintextAddress,
    #[error("invalid OpenBao token: {0}")]
    EmptyToken(#[source] EmptySecretError),
    #[error("failed to build OpenBao HTTP client: {0}")]
    HttpClient(#[source] reqwest::Error),
}

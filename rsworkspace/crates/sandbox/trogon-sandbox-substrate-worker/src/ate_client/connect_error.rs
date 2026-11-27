/// Why the connection to `ate-api-server` could not be established.
#[derive(Debug, thiserror::Error)]
pub enum ConnectError {
    #[error("invalid ate-api-server endpoint: {0}")]
    Endpoint(#[source] tonic::transport::Error),
    #[error("configuring ate-api-server TLS: {0}")]
    Tls(#[source] tonic::transport::Error),
    #[error("connecting to ate-api-server: {0}")]
    Connect(#[source] tonic::transport::Error),
}

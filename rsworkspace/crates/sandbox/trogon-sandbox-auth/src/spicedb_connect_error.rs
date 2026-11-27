use crate::SpiceDbTokenError;

/// Connecting to the permission service failed.
#[derive(Debug, thiserror::Error)]
pub enum SpiceDbConnectError {
    #[error(transparent)]
    Token(#[from] SpiceDbTokenError),
    #[error("could not connect to the permission service: {0}")]
    Transport(#[from] tonic::transport::Error),
}

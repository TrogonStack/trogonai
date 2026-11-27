use oci_client::errors::OciDistributionError;
use trogon_sandbox_contract::DigestError;

use crate::RegistryHostError;

/// A registry operation failed or a registry response was refused.
#[derive(Debug, thiserror::Error)]
pub enum OciError {
    #[error("network error: {0}")]
    Network(#[source] OciDistributionError),

    #[error("artifact not found")]
    NotFound(#[source] OciDistributionError),

    #[error("unauthorized")]
    Unauthorized(#[source] OciDistributionError),

    #[error("digest mismatch: expected {expected}, got {actual}")]
    DigestMismatch { expected: String, actual: String },

    #[error("unsupported media type: {0}")]
    UnsupportedMediaType(String),

    #[error("registry rejected request: {0}")]
    RegistryRejected(String),

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("invalid reference: {0}")]
    InvalidReference(String),

    #[error("invalid digest: {0}")]
    InvalidDigest(#[from] DigestError),

    #[error("invalid registry: {0}")]
    InvalidRegistry(#[from] RegistryHostError),

    #[error("layout error: {0}")]
    Layout(String),

    #[error("configuration error: {0}")]
    Config(String),
}

impl From<OciDistributionError> for OciError {
    fn from(err: OciDistributionError) -> Self {
        match &err {
            OciDistributionError::ImageManifestNotFoundError(_) | OciDistributionError::RegistryError { .. } => {
                Self::NotFound(err)
            }
            OciDistributionError::UnauthorizedError { .. } | OciDistributionError::AuthenticationFailure(_) => {
                Self::Unauthorized(err)
            }
            OciDistributionError::UnsupportedMediaTypeError(message) => Self::UnsupportedMediaType(message.clone()),
            OciDistributionError::ServerError { code, message, .. } => match *code {
                404 => Self::NotFound(err),
                401 | 403 => Self::Unauthorized(err),
                code => Self::RegistryRejected(format!("HTTP {code}: {message}")),
            },
            OciDistributionError::IoError(_) => Self::Io(std::io::Error::other(err.to_string())),
            _ => Self::Network(err),
        }
    }
}

#[cfg(test)]
mod tests;

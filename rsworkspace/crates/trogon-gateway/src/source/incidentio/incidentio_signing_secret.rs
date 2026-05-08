use std::fmt;
use std::sync::Arc;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;

#[derive(Debug)]
pub enum IncidentioSigningSecretError {
    Empty,
    MissingPrefix,
    InvalidBase64(base64::DecodeError),
}

impl fmt::Display for IncidentioSigningSecretError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => f.write_str("signing secret must not be empty"),
            Self::MissingPrefix => f.write_str("signing secret must start with whsec_"),
            Self::InvalidBase64(_) => f.write_str("signing secret must be valid base64"),
        }
    }
}

impl std::error::Error for IncidentioSigningSecretError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::InvalidBase64(err) => Some(err),
            Self::Empty | Self::MissingPrefix => None,
        }
    }
}

#[derive(Clone)]
pub struct IncidentioSigningSecret(Arc<[u8]>);

impl IncidentioSigningSecret {
    pub fn new(secret: impl AsRef<str>) -> Result<Self, IncidentioSigningSecretError> {
        let secret = secret.as_ref();
        if secret.is_empty() {
            return Err(IncidentioSigningSecretError::Empty);
        }
        let secret = secret
            .strip_prefix("whsec_")
            .ok_or(IncidentioSigningSecretError::MissingPrefix)?;
        if secret.is_empty() {
            return Err(IncidentioSigningSecretError::Empty);
        }
        let decoded = STANDARD
            .decode(secret)
            .map_err(IncidentioSigningSecretError::InvalidBase64)?;
        Ok(Self(Arc::from(decoded.into_boxed_slice())))
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }
}

impl fmt::Debug for IncidentioSigningSecret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("IncidentioSigningSecret(****)")
    }
}

#[cfg(test)]
mod tests;

use std::fmt;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;

use crate::constants::BASE64_PREFIX;
use crate::secret_marker::SecretMarker;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ConfigValueError {
    #[error("value with 'base64:' prefix is not valid standard base64")]
    InvalidBase64,
    #[error("value with 'base64:' prefix does not decode to valid UTF-8")]
    InvalidUtf8,
}

#[derive(Clone, PartialEq, Eq)]
pub enum ConfigValue {
    Plain(String),
    Secret { marker: SecretMarker, value: String },
}

impl ConfigValue {
    pub fn parse(raw: impl Into<String>) -> Result<Self, ConfigValueError> {
        let raw = raw.into();
        match SecretMarker::split(&raw) {
            Some((marker, value)) => Ok(Self::Secret {
                value: decode(value)?.unwrap_or_else(|| value.to_string()),
                marker,
            }),
            None => Ok(Self::Plain(decode(&raw)?.unwrap_or(raw))),
        }
    }

    pub fn as_str(&self) -> &str {
        match self {
            Self::Plain(value) | Self::Secret { value, .. } => value,
        }
    }

    pub fn into_string(self) -> String {
        match self {
            Self::Plain(value) | Self::Secret { value, .. } => value,
        }
    }

    pub fn secret_marker(&self) -> Option<&SecretMarker> {
        match self {
            Self::Plain(_) => None,
            Self::Secret { marker, .. } => Some(marker),
        }
    }
}

impl fmt::Debug for ConfigValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Plain(value) => f.debug_tuple("Plain").field(value).finish(),
            Self::Secret { marker, .. } => f
                .debug_struct("Secret")
                .field("marker", marker)
                .field("value", &"[REDACTED]")
                .finish(),
        }
    }
}

fn decode(value: &str) -> Result<Option<String>, ConfigValueError> {
    let Some(encoded) = value.strip_prefix(BASE64_PREFIX) else {
        return Ok(None);
    };
    let compact: String = encoded.chars().filter(|c| !c.is_ascii_whitespace()).collect();
    let bytes = STANDARD.decode(compact).map_err(|_| ConfigValueError::InvalidBase64)?;
    String::from_utf8(bytes)
        .map(Some)
        .map_err(|_| ConfigValueError::InvalidUtf8)
}

#[cfg(test)]
mod tests;

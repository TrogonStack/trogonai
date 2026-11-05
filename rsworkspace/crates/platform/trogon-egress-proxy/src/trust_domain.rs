use crate::constants::TRUST_DOMAIN_MAX_LEN;
use std::fmt;

/// The trust domain a caller certificate must name in its URI SAN.
///
/// Lowercase letters, digits, `.`, `-` and `_`, the character set of a SPIFFE
/// trust domain. Uppercase is rejected rather than folded so the configured
/// value is byte-for-byte what a certificate must carry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrustDomain(String);

impl TrustDomain {
    pub fn new(raw: impl AsRef<str>) -> Result<Self, TrustDomainError> {
        let raw = raw.as_ref();
        if raw.is_empty() {
            return Err(TrustDomainError::Empty);
        }
        if raw.len() > TRUST_DOMAIN_MAX_LEN {
            return Err(TrustDomainError::TooLong {
                max: TRUST_DOMAIN_MAX_LEN,
            });
        }
        if let Some(character) = raw.chars().find(|c| !is_allowed(*c)) {
            return Err(TrustDomainError::InvalidCharacter { character });
        }
        Ok(Self(raw.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

fn is_allowed(c: char) -> bool {
    c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '.' | '-' | '_')
}

impl fmt::Display for TrustDomain {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TrustDomainError {
    #[error("trust domain must not be empty")]
    Empty,
    #[error("trust domain must not be longer than {max} bytes")]
    TooLong { max: usize },
    #[error("trust domain must not contain {character:?}")]
    InvalidCharacter { character: char },
}

#[cfg(test)]
mod tests;

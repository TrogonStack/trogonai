use std::fmt;

use super::constants::MAX_HOST_LEN;
use super::host_syntax::{
    HostSyntaxError, bracketed, has_port, normalize_host, unbracketed_ipv6_literal, validate_ipv6_literal,
    validate_labels,
};

/// The destination host of one credential release, as the caller is about to
/// contact it.
///
/// It exists so a release is always checked against, and audited as, one
/// concrete host: it is never a pattern, never carries a port, and cannot be
/// absent, so a missing destination can never be read as a wildcard.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReleaseHost(String);

impl ReleaseHost {
    pub fn new(value: impl AsRef<str>) -> Result<Self, ReleaseHostError> {
        let value = value.as_ref().trim();
        if value.is_empty() {
            return Err(ReleaseHostError::Empty);
        }
        if value.len() > MAX_HOST_LEN {
            return Err(ReleaseHostError::TooLong(value.len()));
        }

        let normalized = normalize_host(value);
        if let Some(literal) = unbracketed_ipv6_literal(&normalized) {
            return Ok(Self(literal));
        }
        if normalized.starts_with('[') {
            let address = validate_ipv6_literal(&normalized)?;
            return Ok(Self(bracketed(address)));
        }
        if has_port(&normalized) {
            return Err(ReleaseHostError::PortNotAllowed);
        }

        validate_labels(&normalized)?;
        Ok(Self(normalized))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ReleaseHost {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum ReleaseHostError {
    #[error("release host must not be empty")]
    Empty,
    #[error("release host exceeds maximum length: {0}")]
    TooLong(usize),
    #[error("release host label must not be empty")]
    EmptyLabel,
    #[error("release host contains invalid character '{0}'")]
    InvalidCharacter(char),
    #[error("release host must name one host, not a '*' pattern")]
    WildcardNotAllowed,
    #[error("release host must not include a port")]
    PortNotAllowed,
    #[error("release host in brackets must be an IPv6 address, as in '[::1]'")]
    InvalidIpv6Literal,
}

impl From<HostSyntaxError> for ReleaseHostError {
    fn from(error: HostSyntaxError) -> Self {
        match error {
            HostSyntaxError::Empty => Self::Empty,
            HostSyntaxError::EmptyLabel => Self::EmptyLabel,
            HostSyntaxError::InvalidCharacter(ch) => Self::InvalidCharacter(ch),
            HostSyntaxError::MisplacedWildcard => Self::WildcardNotAllowed,
            HostSyntaxError::PortNotAllowed => Self::PortNotAllowed,
            HostSyntaxError::InvalidIpv6Literal => Self::InvalidIpv6Literal,
        }
    }
}

#[cfg(test)]
mod tests;

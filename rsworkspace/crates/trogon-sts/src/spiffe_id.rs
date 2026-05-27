use std::fmt;
use std::str::FromStr;

use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SpiffeId(String);

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum SpiffeIdError {
    #[error("SPIFFE ID must start with spiffe://")]
    MissingScheme,
    #[error("SPIFFE ID missing trust domain")]
    MissingTrustDomain,
    #[error("SPIFFE ID missing path after trust domain")]
    MissingPath,
    #[error("SPIFFE ID trust domain contains invalid characters")]
    InvalidTrustDomain,
    #[error("SPIFFE ID path contains invalid characters")]
    InvalidPath,
}

impl SpiffeId {
    pub fn parse(raw: &str) -> Result<Self, SpiffeIdError> {
        let trimmed = raw.trim();
        let rest = trimmed
            .strip_prefix("spiffe://")
            .ok_or(SpiffeIdError::MissingScheme)?;
        let (trust_domain, path) = rest
            .split_once('/')
            .ok_or(SpiffeIdError::MissingPath)?;
        if trust_domain.is_empty() {
            return Err(SpiffeIdError::MissingTrustDomain);
        }
        if path.is_empty() {
            return Err(SpiffeIdError::MissingPath);
        }
        if !is_valid_trust_domain(trust_domain) {
            return Err(SpiffeIdError::InvalidTrustDomain);
        }
        if !is_valid_path(path) {
            return Err(SpiffeIdError::InvalidPath);
        }
        Ok(Self(format!("spiffe://{trust_domain}/{path}")))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    #[must_use]
    pub fn trust_domain(&self) -> &str {
        self.0
            .strip_prefix("spiffe://")
            .and_then(|r| r.split_once('/'))
            .map(|(td, _)| td)
            .expect("invariant: parsed SPIFFE ID")
    }

    #[must_use]
    pub fn path(&self) -> &str {
        self.0
            .strip_prefix("spiffe://")
            .and_then(|r| r.split_once('/'))
            .map(|(_, p)| p)
            .expect("invariant: parsed SPIFFE ID")
    }

    #[must_use]
    pub fn wkl(&self) -> String {
        self.0.clone()
    }
}

impl fmt::Display for SpiffeId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl FromStr for SpiffeId {
    type Err = SpiffeIdError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s)
    }
}

fn is_valid_trust_domain(s: &str) -> bool {
    s.chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_')
}

fn is_valid_path(s: &str) -> bool {
    !s.contains("//")
        && s.chars()
            .all(|c| c.is_ascii() && !c.is_control() && c != ' ' && c != '#')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_valid_spiffe_id() {
        let id = SpiffeId::parse("spiffe://acme.local/ns/prod/sa/oncall-agent").expect("parse");
        assert_eq!(id.trust_domain(), "acme.local");
        assert_eq!(id.path(), "ns/prod/sa/oncall-agent");
        assert_eq!(id.as_str(), "spiffe://acme.local/ns/prod/sa/oncall-agent");
    }

    #[test]
    fn rejects_missing_scheme() {
        assert_eq!(
            SpiffeId::parse("acme.local/ns/x").unwrap_err(),
            SpiffeIdError::MissingScheme
        );
    }

    #[test]
    fn rejects_missing_path() {
        assert_eq!(
            SpiffeId::parse("spiffe://acme.local").unwrap_err(),
            SpiffeIdError::MissingPath
        );
    }

    #[test]
    fn rejects_empty_trust_domain() {
        assert_eq!(
            SpiffeId::parse("spiffe:///ns/x").unwrap_err(),
            SpiffeIdError::MissingTrustDomain
        );
    }
}

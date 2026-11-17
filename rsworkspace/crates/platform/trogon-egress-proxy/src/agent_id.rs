use crate::constants::AGENT_ID_MAX_LEN;
use std::fmt;

/// The identity the proxy looks a permit up under.
///
/// An `AgentId` is opaque to the proxy. It is restricted to characters that
/// need no escaping inside a URI path segment, so the certificate alias of
/// [`crate::agent_uri`] can carry it verbatim and map back to the same value.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct AgentId(String);

impl AgentId {
    pub fn new(raw: impl AsRef<str>) -> Result<Self, AgentIdError> {
        let raw = raw.as_ref();
        if raw.is_empty() {
            return Err(AgentIdError::Empty);
        }
        if raw.len() > AGENT_ID_MAX_LEN {
            return Err(AgentIdError::TooLong { max: AGENT_ID_MAX_LEN });
        }
        if let Some(character) = raw.chars().find(|c| !is_allowed(*c)) {
            return Err(AgentIdError::InvalidCharacter { character });
        }
        Ok(Self(raw.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

fn is_allowed(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | ':')
}

impl fmt::Display for AgentId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AgentIdError {
    #[error("agent id must not be empty")]
    Empty,
    #[error("agent id must not be longer than {max} bytes")]
    TooLong { max: usize },
    #[error("agent id must not contain {character:?}")]
    InvalidCharacter { character: char },
}

#[cfg(test)]
mod tests;

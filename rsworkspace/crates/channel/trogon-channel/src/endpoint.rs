#[cfg(test)]
mod tests;

use crate::safe_token::{SafeToken, SafeTokenError};
use serde::{Deserialize, Deserializer, Serialize};

/// Why an endpoint or principal identifier could not be constructed.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum EndpointError {
    #[error("token must not be empty")]
    Empty,
    #[error("token contains invalid character: {0:?}")]
    InvalidCharacter(char),
}

impl From<SafeTokenError> for EndpointError {
    fn from(error: SafeTokenError) -> Self {
        match error {
            SafeTokenError::Empty => Self::Empty,
            SafeTokenError::InvalidCharacter(c) => Self::InvalidCharacter(c),
        }
    }
}

/// Wire shape for an [`Endpoint`]. Converted through [`Endpoint::new`] so each
/// token is validated independently before the domain value exists.
#[derive(Debug, Deserialize)]
struct EndpointWire {
    channel: String,
    account: String,
    peer: String,
}

/// Where a message arrives and leaves: a platform, a bot account on it, and a
/// peer on that platform. Many endpoints can point at one conversation.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
pub struct Endpoint {
    channel: SafeToken,
    account: SafeToken,
    peer: SafeToken,
}

impl Endpoint {
    pub fn new(
        channel: impl Into<String>,
        account: impl Into<String>,
        peer: impl Into<String>,
    ) -> Result<Self, EndpointError> {
        Ok(Self {
            channel: SafeToken::new(channel)?,
            account: SafeToken::new(account)?,
            peer: SafeToken::new(peer)?,
        })
    }

    pub fn channel(&self) -> &str {
        self.channel.as_str()
    }

    pub fn account(&self) -> &str {
        self.account.as_str()
    }

    pub fn peer(&self) -> &str {
        self.peer.as_str()
    }

    /// Stable KV key for this endpoint (also a valid subject suffix).
    pub fn kv_key(&self) -> String {
        format!("{}.{}.{}", self.channel, self.account, self.peer)
    }
}

impl<'de> Deserialize<'de> for Endpoint {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let wire = EndpointWire::deserialize(deserializer)?;
        Self::new(wire.channel, wire.account, wire.peer).map_err(serde::de::Error::custom)
    }
}

impl std::fmt::Display for Endpoint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.kv_key())
    }
}

/// The human behind one or more endpoints. Cross-channel by design: linking a
/// Telegram user and a Discord user to the same principal is what lets one
/// conversation continue across channels.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
pub struct PrincipalId(SafeToken);

impl PrincipalId {
    pub fn new(id: impl Into<String>) -> Result<Self, EndpointError> {
        Ok(Self(SafeToken::new(id)?))
    }

    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

impl<'de> Deserialize<'de> for PrincipalId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let raw = String::deserialize(deserializer)?;
        Self::new(raw).map_err(serde::de::Error::custom)
    }
}

impl std::fmt::Display for PrincipalId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

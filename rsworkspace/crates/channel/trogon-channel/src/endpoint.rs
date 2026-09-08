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

/// The half of an endpoint a bridge process is: the channel it speaks for and
/// the account it speaks as. Both come from the deployment, so both are checked
/// once here, which leaves [`ChannelAccount::endpoint_for`] with nothing left to
/// reject.
///
/// This exists because the alternative validated them per message, which is the
/// wrong moment to find out. A bridge configured with an account that is not a
/// token started, read every update, failed to name an endpoint for any of them,
/// and acked them all: an operator saw a healthy process answering nobody.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChannelAccount {
    channel: SafeToken,
    account: SafeToken,
}

impl ChannelAccount {
    pub fn new(channel: impl Into<String>, account: impl Into<String>) -> Result<Self, EndpointError> {
        Ok(Self {
            channel: SafeToken::new(channel)?,
            account: SafeToken::new(account)?,
        })
    }

    /// The account token, for the platform-facing uses that are not endpoints:
    /// a Telegram command may be addressed as `/new@account`.
    pub fn account(&self) -> &str {
        self.account.as_str()
    }

    /// Where one peer on this account is reached.
    pub fn endpoint_for(&self, peer: &SafeToken) -> Endpoint {
        Endpoint {
            channel: self.channel.clone(),
            account: self.account.clone(),
            peer: peer.clone(),
        }
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

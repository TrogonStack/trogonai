use serde::{Deserialize, Serialize};

/// Characters permitted in endpoint tokens: the intersection of what NATS KV
/// keys accept and what NATS subject tokens accept, so an endpoint can address
/// both a KV entry and (after extraction) a subject without re-encoding.
fn is_safe_token(token: &str) -> bool {
    !token.is_empty()
        && token
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '='))
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum EndpointError {
    #[error("endpoint token {0:?} is empty or contains unsafe characters")]
    UnsafeToken(String),
}

/// Where a message arrives and leaves: a platform, a bot account on it, and a
/// chat/user on that platform. Many endpoints can point at one conversation.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Endpoint {
    channel: String,
    account: String,
    peer: String,
}

impl Endpoint {
    pub fn new(
        channel: impl Into<String>,
        account: impl Into<String>,
        peer: impl Into<String>,
    ) -> Result<Self, EndpointError> {
        let channel = channel.into();
        let account = account.into();
        let peer = peer.into();
        for token in [&channel, &account, &peer] {
            if !is_safe_token(token) {
                return Err(EndpointError::UnsafeToken(token.clone()));
            }
        }
        Ok(Self { channel, account, peer })
    }

    pub fn channel(&self) -> &str {
        &self.channel
    }

    pub fn account(&self) -> &str {
        &self.account
    }

    pub fn peer(&self) -> &str {
        &self.peer
    }

    /// Stable KV key for this endpoint (also a valid subject suffix).
    pub fn kv_key(&self) -> String {
        format!("{}.{}.{}", self.channel, self.account, self.peer)
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
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PrincipalId(String);

impl PrincipalId {
    pub fn new(id: impl Into<String>) -> Result<Self, EndpointError> {
        let id = id.into();
        if !is_safe_token(&id) {
            return Err(EndpointError::UnsafeToken(id));
        }
        Ok(Self(id))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for PrincipalId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoint_accepts_negative_telegram_chat_ids() {
        let e = Endpoint::new("telegram", "mybot", "-1001234567890").expect("valid");
        assert_eq!(e.kv_key(), "telegram.mybot.-1001234567890");
    }

    #[test]
    fn endpoint_rejects_unsafe_tokens() {
        assert!(Endpoint::new("telegram", "my bot", "1").is_err());
        assert!(Endpoint::new("", "mybot", "1").is_err());
        assert!(Endpoint::new("telegram", "mybot", "a.b").is_err());
    }
}

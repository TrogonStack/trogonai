use std::fmt;

#[derive(Debug)]
pub enum BridgeError {
    MissingAuthorization,
    Utf8Body(std::str::Utf8Error),
    Deserialize(serde_json::Error),
    Serialize(serde_json::Error),
    MissingAgentHeader,
    MissingJsonRpcMethod,
    Mint(String),
    NatsPublish(String),
    JetStreamConsume(String),
    UpstreamHttps(String),
    MissingJsonRpcId,
    StreamingParams(String),
    JsonRpcUpstream(String),
    CatalogRegistration(String),
    InvalidAgent(String),
}

impl fmt::Display for BridgeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingAuthorization => write!(f, "missing Authorization header"),
            Self::Utf8Body(e) => write!(f, "request body was not UTF-8: {e}"),
            Self::Deserialize(e) => write!(f, "failed to deserialize JSON-RPC body: {e}"),
            Self::Serialize(e) => write!(f, "failed to serialize bridge response: {e}"),
            Self::MissingAgentHeader => write!(f, "missing X-A2A-Agent-Id header"),
            Self::MissingJsonRpcMethod => write!(f, "JSON-RPC body missing method"),
            Self::Mint(msg) => write!(f, "auth callout mint failed: {msg}"),
            Self::NatsPublish(msg) => write!(f, "NATS gateway publish failed: {msg}"),
            Self::JetStreamConsume(msg) => write!(f, "JetStream SSE consumer attach failed: {msg}"),
            Self::UpstreamHttps(msg) => write!(f, "HTTPS upstream forward failed: {msg}"),
            Self::MissingJsonRpcId => write!(f, "JSON-RPC streaming request missing usable id"),
            Self::StreamingParams(msg) => write!(f, "invalid streaming RPC params: {msg}"),
            Self::JsonRpcUpstream(msg) => write!(f, "gateway unary returned JSON-RPC error: {msg}"),
            Self::CatalogRegistration(msg) => write!(f, "catalog registration publish failed: {msg}"),
            Self::InvalidAgent(msg) => write!(f, "invalid agent identifier: {msg}"),
        }
    }
}

impl std::error::Error for BridgeError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Utf8Body(e) => Some(e),
            Self::Deserialize(e) | Self::Serialize(e) => Some(e),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error;

    use super::*;

    #[test]
    fn display_mint() {
        assert!(
            BridgeError::Mint("unavailable".into())
                .to_string()
                .contains("auth callout mint")
        );
    }

    #[test]
    fn source_for_deserialize() {
        let e = BridgeError::Deserialize(serde_json::from_str::<serde_json::Value>("]").unwrap_err());
        assert!(e.source().is_some());
    }

    #[test]
    fn source_for_missing_authorization_none() {
        assert!(BridgeError::MissingAuthorization.source().is_none());
    }
}

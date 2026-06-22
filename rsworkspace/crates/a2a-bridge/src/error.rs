#[allow(dead_code)]
#[derive(Debug, thiserror::Error)]
pub enum BridgeError {
    #[error("missing Authorization header")]
    MissingAuthorization,
    #[error("request body was not UTF-8: {0}")]
    Utf8Body(#[source] std::str::Utf8Error),
    #[error("failed to deserialize JSON-RPC body: {0}")]
    Deserialize(#[source] serde_json::Error),
    #[error("failed to serialize bridge response: {0}")]
    Serialize(#[source] serde_json::Error),
    #[error("missing X-A2A-Agent-Id header")]
    MissingAgentHeader,
    #[error("JSON-RPC body missing method")]
    MissingJsonRpcMethod,
    #[error("auth callout mint failed: {0}")]
    Mint(String),
    #[error("NATS gateway publish failed: {0}")]
    NatsPublish(String),
    #[error("JetStream SSE consumer attach failed: {0}")]
    JetStreamConsume(String),
    #[error("HTTPS upstream forward failed: {0}")]
    UpstreamHttps(String),
    #[error("JSON-RPC streaming request missing usable id")]
    MissingJsonRpcId,
    #[error("invalid streaming RPC params: {0}")]
    StreamingParams(String),
    #[error("gateway unary returned JSON-RPC error: {0}")]
    JsonRpcUpstream(String),
    #[error("catalog registration publish failed: {0}")]
    CatalogRegistration(String),
    #[error("invalid agent identifier: {0}")]
    InvalidAgent(String),
    #[error("failed to build HTTP response: {0}")]
    ResponseBuild(String),
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

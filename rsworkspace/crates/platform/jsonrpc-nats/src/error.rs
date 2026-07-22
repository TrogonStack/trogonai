use thiserror::Error;

#[derive(Debug, Error)]
pub enum CodecError {
    #[error("failed to serialize JSON-RPC payload")]
    Serialize(#[source] serde_json::Error),
    #[error("failed to deserialize JSON-RPC payload")]
    Deserialize(#[source] serde_json::Error),
    #[error("unsupported JSON-RPC version: expected \"2.0\", found {found:?}")]
    UnsupportedVersion { found: Option<String> },
    #[error("invalid Jsonrpc-Id header value: {value}")]
    InvalidIdHeader { value: String },
    #[error("invalid Jsonrpc-Error-Code header value: {value}")]
    InvalidErrorCodeHeader { value: String },
    #[error("response has neither result body nor Jsonrpc-Error-Code header")]
    AmbiguousResponse,
    #[error("request is missing a method")]
    MissingMethod,
    #[error("notification must not carry Jsonrpc-Id")]
    NotificationWithId,
    #[error("request must carry a non-null Jsonrpc-Id")]
    RequestWithoutId,
}

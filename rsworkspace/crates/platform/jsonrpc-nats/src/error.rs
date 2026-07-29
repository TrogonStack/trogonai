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
    #[error("NATS request subject is missing its method projection")]
    MissingMethodProjection,
    #[error("NATS method projection {projected:?} does not match JSON-RPC body method {actual:?}")]
    MethodProjectionMismatch { projected: String, actual: String },
    #[error("response unexpectedly carries NATS method projection {projected:?}")]
    UnexpectedMethodProjection { projected: String },
    #[error("NATS id projection {projected} does not match JSON-RPC body id {actual}")]
    IdProjectionMismatch { projected: String, actual: String },
    #[error("NATS error-code projection {projected} does not match JSON-RPC body error code {actual}")]
    ErrorCodeProjectionMismatch { projected: i32, actual: i32 },
    #[error("non-error JSON-RPC body unexpectedly carries NATS error-code projection {projected}")]
    UnexpectedErrorCodeProjection { projected: i32 },
    #[error("expected JSON-RPC {expected}, found {actual}")]
    DirectionMismatch {
        expected: &'static str,
        actual: &'static str,
    },
    #[error("canonical JSON-RPC method must be a string")]
    InvalidCanonicalMethod,
    #[error("canonical JSON-RPC params must be an object or array when present")]
    InvalidCanonicalParams,
    #[error("canonical JSON-RPC request or notification must not contain result or error")]
    CanonicalRequestWithResponseMembers,
    #[error("canonical JSON-RPC response must not contain method or params")]
    CanonicalResponseWithRequestMembers,
    #[error("canonical JSON-RPC response is missing id")]
    CanonicalResponseWithoutId,
    #[error("canonical JSON-RPC success response must not use a null id")]
    CanonicalSuccessWithNullId,
    #[error("canonical JSON-RPC response must contain exactly one of result or error")]
    InvalidCanonicalResponseShape,
    #[error("canonical JSON-RPC error must be an object")]
    InvalidCanonicalErrorObject,
    #[error("canonical JSON-RPC error code must be an i32 integer")]
    InvalidCanonicalErrorCode,
    #[error("canonical JSON-RPC error message must be a string")]
    InvalidCanonicalErrorMessage,
    #[error("notification must not carry Jsonrpc-Id")]
    NotificationWithId,
    #[error("request must carry a non-null Jsonrpc-Id")]
    RequestWithoutId,
}

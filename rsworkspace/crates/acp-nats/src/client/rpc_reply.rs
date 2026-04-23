pub use crate::constants::{CONTENT_TYPE_JSON, CONTENT_TYPE_PLAIN};
use crate::nats::{FlushClient, PublishClient, headers_with_trace_context};
use agent_client_protocol::{Error, ErrorCode, RequestId, Response};
use bytes::Bytes;
use tracing::warn;
use trogon_std::JsonSerialize;

pub fn error_response_fallback_bytes<S: JsonSerialize>(serializer: &S) -> (Bytes, &'static str) {
    match serializer.to_vec(&Response::<()>::Error {
        id: RequestId::Null,
        error: Error::new(-32603, "Internal error"),
    }) {
        Ok(v) => (Bytes::from(v), CONTENT_TYPE_JSON),
        Err(e) => {
            warn!(
                error = %e,
                "Fallback JSON serialization failed, response may not be valid JSON-RPC"
            );
            (Bytes::from("Internal error"), CONTENT_TYPE_PLAIN)
        }
    }
}

pub async fn publish_reply<N: PublishClient + FlushClient>(
    nats: &N,
    reply_to: &str,
    bytes: Bytes,
    content_type: &str,
    context: &str,
) {
    let mut headers = headers_with_trace_context();
    headers.insert("Content-Type", content_type);
    if let Err(e) = nats.publish_with_headers(reply_to.to_string(), headers, bytes).await {
        warn!(error = %e, "Failed to publish {}", context);
    }
    if let Err(e) = nats.flush().await {
        warn!(error = %e, "Failed to flush {}", context);
    }
}

pub fn error_response_bytes<S: JsonSerialize>(
    serializer: &S,
    request_id: RequestId,
    code: ErrorCode,
    message: &str,
) -> (Bytes, &'static str) {
    let response = Response::<()>::Error {
        id: request_id,
        error: Error::new(i32::from(code), message),
    };
    match serializer.to_vec(&response) {
        Ok(v) => (Bytes::from(v), CONTENT_TYPE_JSON),
        Err(e) => {
            warn!(error = %e, "JSON serialization failed, using fallback error");
            error_response_fallback_bytes(serializer)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_client_protocol::{ErrorCode, RequestId};
    use trogon_std::{FailNextSerialize, StdJsonSerialize};

    #[test]
    fn error_response_bytes_first_fallback_uses_null_id() {
        let mock = FailNextSerialize::new(1);
        let (bytes, content_type) =
            error_response_bytes(&mock, RequestId::Number(42), ErrorCode::InvalidParams, "test message");
        assert_eq!(content_type, "application/json");
        let parsed: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(parsed["id"], serde_json::Value::Null);
        assert_eq!(parsed["error"]["code"], -32603);
    }

    #[test]
    fn error_response_bytes_last_resort_returns_plain_text() {
        let mock = FailNextSerialize::new(2);
        let (bytes, content_type) = error_response_bytes(&mock, RequestId::Number(1), ErrorCode::InternalError, "msg");
        assert_eq!(content_type, "text/plain");
        assert_eq!(bytes.as_ref(), b"Internal error");
    }

    #[test]
    fn error_response_fallback_bytes_std_serializer_returns_json() {
        let (bytes, content_type) = error_response_fallback_bytes(&StdJsonSerialize);
        assert_eq!(content_type, "application/json");
        let parsed: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(parsed["id"], serde_json::Value::Null);
        assert_eq!(parsed["error"]["code"], -32603);
    }
}

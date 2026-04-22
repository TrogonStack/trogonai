//! Pass-thru helpers for JSON-RPC using agent_client_protocol types.

use agent_client_protocol::{Request, RequestId};

/// Extract the request ID from a JSON-RPC payload without fully deserializing params.
pub fn extract_request_id(payload: &[u8]) -> RequestId {
    serde_json::from_slice::<Request<serde_json::Value>>(payload)
        .ok()
        .map(|r| r.id)
        .unwrap_or(RequestId::Null)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_number_id() {
        let payload = br#"{"id":42,"method":"initialize","params":{}}"#;
        assert_eq!(extract_request_id(payload), RequestId::Number(42));
    }

    #[test]
    fn extract_string_id() {
        let payload = br#"{"id":"req-abc","method":"prompt","params":{}}"#;
        assert_eq!(
            extract_request_id(payload),
            RequestId::Str("req-abc".to_string())
        );
    }

    #[test]
    fn extract_null_id_explicit() {
        let payload = br#"{"id":null,"method":"prompt","params":{}}"#;
        assert_eq!(extract_request_id(payload), RequestId::Null);
    }

    #[test]
    fn extract_invalid_json_returns_null() {
        assert_eq!(extract_request_id(b"not json at all"), RequestId::Null);
    }

    #[test]
    fn extract_empty_bytes_returns_null() {
        assert_eq!(extract_request_id(b""), RequestId::Null);
    }

    #[test]
    fn extract_missing_id_field_returns_null() {
        // Valid JSON but no "id" field — deserialize fails, falls back to Null
        let payload = br#"{"method":"prompt","params":{}}"#;
        assert_eq!(extract_request_id(payload), RequestId::Null);
    }

    #[test]
    fn extract_negative_number_id() {
        let payload = br#"{"id":-1,"method":"cancel","params":{}}"#;
        assert_eq!(extract_request_id(payload), RequestId::Number(-1));
    }

    #[test]
    fn extract_zero_id() {
        let payload = br#"{"id":0,"method":"initialize","params":{}}"#;
        assert_eq!(extract_request_id(payload), RequestId::Number(0));
    }

    #[test]
    fn extract_ignores_params_content() {
        // Large/complex params must not break ID extraction
        let payload = br#"{"id":99,"method":"prompt","params":{"messages":[{"role":"user","content":"hello"}],"max_tokens":1024}}"#;
        assert_eq!(extract_request_id(payload), RequestId::Number(99));
    }
}

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
    fn extracts_numeric_id() {
        let payload = br#"{"jsonrpc":"2.0","id":42,"method":"initialize","params":{}}"#;
        assert_eq!(extract_request_id(payload), RequestId::Number(42));
    }

    #[test]
    fn extracts_zero_id() {
        let payload = br#"{"jsonrpc":"2.0","id":0,"method":"prompt","params":{}}"#;
        assert_eq!(extract_request_id(payload), RequestId::Number(0));
    }

    #[test]
    fn returns_null_for_invalid_json() {
        assert_eq!(extract_request_id(b"not json at all"), RequestId::Null);
    }

    #[test]
    fn returns_null_for_empty_input() {
        assert_eq!(extract_request_id(b""), RequestId::Null);
    }

    #[test]
    fn returns_null_for_missing_method_field() {
        // Missing "method" makes it fail to deserialize as Request
        let payload = br#"{"jsonrpc":"2.0","id":1,"params":{}}"#;
        assert_eq!(extract_request_id(payload), RequestId::Null);
    }

    #[test]
    fn returns_null_for_null_id_field() {
        let payload = br#"{"jsonrpc":"2.0","id":null,"method":"cancel","params":{}}"#;
        assert_eq!(extract_request_id(payload), RequestId::Null);
    }
}

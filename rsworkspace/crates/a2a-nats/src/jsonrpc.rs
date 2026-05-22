//! JSON-RPC envelope helpers for the A2A binding.
//!
//! A2A method dispatch lives at the NATS subject level — one subject per method, so the
//! JSON-RPC `method` field is redundant on the wire and we don't require it. What we do
//! need: extract the request `id` from a raw payload without paying full deserialization
//! cost, so we can route responses back to the right caller inbox.

use serde_json::Value;

/// Minimal JSON-RPC id, mirroring the subset A2A uses.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
#[serde(untagged)]
pub enum JsonRpcId {
    Number(i64),
    String(String),
    Null,
}

impl std::fmt::Display for JsonRpcId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Number(n) => write!(f, "{n}"),
            Self::String(s) => f.write_str(s),
            Self::Null => f.write_str("null"),
        }
    }
}

/// Extracts the JSON-RPC id from a raw payload, returning `None` if absent or malformed.
///
/// Distinguishes `id: null` (valid JSON-RPC, returns `Some(JsonRpcId::Null)`) from
/// `id` absent entirely (returns `None`).
pub fn extract_request_id(raw: &[u8]) -> Option<JsonRpcId> {
    let value: Value = serde_json::from_slice(raw).ok()?;
    let id = value.as_object()?.get("id")?;
    match id {
        Value::Number(n) => n.as_i64().map(JsonRpcId::Number),
        Value::String(s) => Some(JsonRpcId::String(s.clone())),
        Value::Null => Some(JsonRpcId::Null),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_numeric_id() {
        let raw = br#"{"jsonrpc":"2.0","id":42,"method":"message/send","params":{}}"#;
        assert_eq!(extract_request_id(raw), Some(JsonRpcId::Number(42)));
    }

    #[test]
    fn extract_string_id() {
        let raw = br#"{"id":"abc-123"}"#;
        assert_eq!(extract_request_id(raw), Some(JsonRpcId::String("abc-123".into())));
    }

    #[test]
    fn extract_null_id() {
        let raw = br#"{"id":null}"#;
        assert_eq!(extract_request_id(raw), Some(JsonRpcId::Null));
    }

    #[test]
    fn missing_id_returns_none() {
        let raw = br#"{"jsonrpc":"2.0"}"#;
        assert_eq!(extract_request_id(raw), None);
    }

    #[test]
    fn malformed_payload_returns_none() {
        assert_eq!(extract_request_id(b"not json"), None);
    }

    #[test]
    fn boolean_id_returns_none() {
        let raw = br#"{"id":true}"#;
        assert_eq!(extract_request_id(raw), None);
    }

    #[test]
    fn id_roundtrips_through_serde() {
        let id = JsonRpcId::String("x".into());
        let bytes = serde_json::to_vec(&id).unwrap();
        let back: JsonRpcId = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(id, back);
    }
}

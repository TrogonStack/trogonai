//! Pass-thru helpers for JSON-RPC using agent_client_protocol types.

use agent_client_protocol::{Request, RequestId};

/// Extract the request ID from a JSON-RPC payload without fully deserializing params.
pub fn extract_request_id(payload: &[u8]) -> RequestId {
    serde_json::from_slice::<Request<serde_json::Value>>(payload)
        .ok()
        .map(|r| r.id)
        .unwrap_or(RequestId::Null)
}

use agent_client_protocol::SessionId;
use serde::{Deserialize, Serialize};

/// Published by the bridge after successfully sending a `session/new` or
/// `session/load` response to the client. This signals the backend that
/// it can now safely send notifications, ensuring proper message ordering.
///
/// See the upstream RFD: <https://github.com/agentclientprotocol/agent-client-protocol/pull/419>
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExtSessionReady {
    pub session_id: SessionId,
}

impl ExtSessionReady {
    pub fn new(session_id: SessionId) -> Self {
        Self { session_id }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_stores_session_id() {
        let id = SessionId::from("my-session-1");
        let msg = ExtSessionReady::new(id.clone());
        assert_eq!(msg.session_id, id);
    }

    #[test]
    fn serializes_to_json_with_session_id_field() {
        let msg = ExtSessionReady::new(SessionId::from("sess-42"));
        let v = serde_json::to_value(&msg).unwrap();
        assert_eq!(v["session_id"], "sess-42");
    }

    #[test]
    fn deserializes_from_json() {
        let json = r#"{"session_id":"sess-abc"}"#;
        let msg: ExtSessionReady = serde_json::from_str(json).unwrap();
        assert_eq!(msg.session_id, SessionId::from("sess-abc"));
    }

    #[test]
    fn roundtrip_serialize_deserialize() {
        let original = ExtSessionReady::new(SessionId::from("roundtrip-session"));
        let json = serde_json::to_string(&original).unwrap();
        let decoded: ExtSessionReady = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.session_id, original.session_id);
    }

    #[test]
    fn clone_produces_equal_value() {
        let msg = ExtSessionReady::new(SessionId::from("clone-test"));
        let cloned = msg.clone();
        assert_eq!(cloned.session_id, msg.session_id);
    }
}

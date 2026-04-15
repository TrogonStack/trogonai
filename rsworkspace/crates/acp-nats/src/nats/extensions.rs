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

    fn session_id(s: &str) -> SessionId {
        SessionId::new(s.to_string())
    }

    #[test]
    fn new_stores_session_id() {
        let id = session_id("session-abc");
        let msg = ExtSessionReady::new(id.clone());
        assert_eq!(msg.session_id, id);
    }

    #[test]
    fn round_trips_through_json() {
        let id = session_id("session-xyz");
        let msg = ExtSessionReady::new(id.clone());
        let json = serde_json::to_string(&msg).unwrap();
        let decoded: ExtSessionReady = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.session_id, id);
    }

    #[test]
    fn clone_produces_equal_value() {
        let msg = ExtSessionReady::new(session_id("session-clone"));
        let cloned = msg.clone();
        assert_eq!(cloned.session_id, msg.session_id);
    }
}

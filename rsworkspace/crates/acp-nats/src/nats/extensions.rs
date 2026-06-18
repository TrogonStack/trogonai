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

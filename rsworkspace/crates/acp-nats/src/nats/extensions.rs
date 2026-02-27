//! Extension message types for internal coordination between bridge and backend.
//!
//! These are not part of the ACP protocol but are used for internal coordination
//! via NATS extension subjects.

use agent_client_protocol::SessionId;
use serde::{Deserialize, Serialize};

/// Published by the bridge after successfully sending a `session/new` or
/// `session/load` response to the client. This signals the backend that
/// it can now safely send notifications, ensuring proper message ordering.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExtSessionReady {
    pub session_id: SessionId,
}

impl ExtSessionReady {
    pub fn new(session_id: SessionId) -> Self {
        Self { session_id }
    }
}

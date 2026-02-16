//! Extension message types for internal coordination between bridge and backend.
//!
//! These are not part of the ACP protocol but are used for internal coordination
//! via NATS extension subjects.

use serde::{Deserialize, Serialize};

/// Session ready extension message.
///
/// Published by the Rust bridge after successfully sending a `session/new` or
/// `session/load` response to Zed. This signals the TypeScript backend that
/// it can now safely send notifications, ensuring proper message ordering.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionReady {
    /// The session ID that is ready
    pub session_id: String,
}

impl SessionReady {
    /// Create a new SessionReady message
    pub fn new(session_id: String) -> Self {
        Self { session_id }
    }
}

use serde::{Deserialize, Serialize};
use trogon_sandbox_contract::LeaseId;

use crate::SshSessionId;

/// Ends a session and drops the key it attached with.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct SshDetachRequest {
    pub session_id: SshSessionId,
    pub lease_id: LeaseId,
    #[serde(default)]
    pub reason: String,
}

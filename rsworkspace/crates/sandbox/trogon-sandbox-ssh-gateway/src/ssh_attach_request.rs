use std::num::NonZeroU32;

use serde::{Deserialize, Serialize};
use trogon_sandbox_contract::LeaseId;

use crate::SshSessionId;

/// Asks the worker holding `lease_id` to accept an SSH session.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct SshAttachRequest {
    pub session_id: SshSessionId,
    pub lease_id: LeaseId,
    /// OpenSSH authorized_keys line for the ephemeral session key.
    pub public_key: String,
    pub ttl_seconds: NonZeroU32,
}

use std::time::SystemTime;

use trogon_sandbox_contract::CredentialId;

/// A stored credential without its material.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct CredentialSummary {
    pub credential_id: CredentialId,
    pub last_rotated: SystemTime,
    pub version: u32,
}

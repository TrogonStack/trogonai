use trogon_sandbox_contract::{LeaseId, TokenId};

use crate::SshSessionId;

/// Tracks a credential broker session token for revocation on detach.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct SessionCredential {
    pub token_id: TokenId,
    pub lease_id: LeaseId,
    pub session_id: SshSessionId,
}

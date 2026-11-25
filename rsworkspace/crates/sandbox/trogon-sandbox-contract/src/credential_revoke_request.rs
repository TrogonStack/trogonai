use serde::{Deserialize, Serialize};

use crate::TokenId;

/// A request to revoke one issued token. An omitted `reason` reads as empty.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct CredentialRevokeRequest {
    pub token_id: TokenId,
    #[serde(default)]
    pub reason: String,
}

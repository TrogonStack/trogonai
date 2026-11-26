use std::time::SystemTime;

use serde::{Deserialize, Serialize};

use crate::{ShortLivedToken, TokenId};

/// The token the credential broker issued and when it expires.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct CredentialIssueResponse {
    pub token_id: TokenId,
    pub token: ShortLivedToken,
    pub expires_at: SystemTime,
}

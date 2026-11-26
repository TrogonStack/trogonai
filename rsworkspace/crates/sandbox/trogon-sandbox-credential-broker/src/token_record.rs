use std::num::NonZeroU32;
use std::time::{Duration, SystemTime};

use serde::{Deserialize, Serialize};
use trogon_sandbox_contract::{CredentialError, CredentialId, CredentialScope, ShortLivedToken, TokenId};
use uuid::Uuid;

/// An issued token as a token store keeps it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenRecord {
    pub credential_id: CredentialId,
    pub scope: CredentialScope,
    pub token_value: ShortLivedToken,
    pub expires_at: SystemTime,
    pub revoked: bool,
}

impl TokenRecord {
    /// A fresh, unrevoked record that expires `ttl_seconds` from now.
    pub(crate) fn new(
        credential_id: &CredentialId,
        scope: &CredentialScope,
        token_value: ShortLivedToken,
        ttl_seconds: NonZeroU32,
    ) -> Self {
        Self {
            credential_id: credential_id.clone(),
            scope: scope.clone(),
            token_value,
            expires_at: SystemTime::now() + Duration::from_secs(u64::from(ttl_seconds.get())),
            revoked: false,
        }
    }

    /// The record when it is still usable, or the error a caller sees for a revoked or expired token.
    pub(crate) fn usable(self, token_id: &TokenId, now: SystemTime) -> Result<Self, CredentialError> {
        if self.revoked {
            return Err(CredentialError::TokenRevoked {
                token_id: token_id.clone(),
            });
        }
        if now >= self.expires_at {
            return Err(CredentialError::TokenExpired {
                token_id: token_id.clone(),
            });
        }
        Ok(self)
    }
}

/// A new, time-ordered token identifier.
pub(crate) fn new_token_id() -> TokenId {
    TokenId::new(Uuid::now_v7().simple().to_string())
}

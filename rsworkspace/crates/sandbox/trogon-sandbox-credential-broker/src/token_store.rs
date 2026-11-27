use std::num::NonZeroU32;

use async_trait::async_trait;
use trogon_sandbox_contract::{
    CredentialError, CredentialId, CredentialIssueResponse, CredentialScope, ShortLivedToken, TokenId,
};

use crate::{TokenRecord, TokenStoreError};

/// Where issued tokens live until they expire or are revoked.
#[async_trait]
pub trait TokenStore: Send + Sync {
    /// Records a token carrying `token_value` for `ttl_seconds` and returns it with its new identifier.
    async fn issue(
        &self,
        credential_id: &CredentialId,
        scope: &CredentialScope,
        token_value: ShortLivedToken,
        ttl_seconds: NonZeroU32,
    ) -> Result<CredentialIssueResponse, TokenStoreError>;

    /// The record of a token that is still usable.
    async fn validate(&self, token_id: &TokenId) -> Result<TokenRecord, CredentialError>;

    /// Marks the token revoked. A token that is unknown, expired or already revoked is left alone.
    async fn revoke(&self, token_id: &TokenId) -> Result<(), TokenStoreError>;
}

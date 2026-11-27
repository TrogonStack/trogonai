use std::collections::HashMap;
use std::num::NonZeroU32;
use std::time::SystemTime;

use async_trait::async_trait;
use tokio::sync::Mutex;
use trogon_sandbox_contract::{
    CredentialError, CredentialId, CredentialIssueResponse, CredentialScope, ShortLivedToken, TokenId,
};

use crate::token_record::new_token_id;
use crate::{TokenRecord, TokenStore, TokenStoreError};

/// Tokens held in process memory.
#[derive(Default)]
pub struct InMemoryTokenStore {
    records: Mutex<HashMap<TokenId, TokenRecord>>,
}

impl InMemoryTokenStore {
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl TokenStore for InMemoryTokenStore {
    async fn issue(
        &self,
        credential_id: &CredentialId,
        scope: &CredentialScope,
        token_value: ShortLivedToken,
        ttl_seconds: NonZeroU32,
    ) -> Result<CredentialIssueResponse, TokenStoreError> {
        let token_id = new_token_id();
        let record = TokenRecord::new(credential_id, scope, token_value.clone(), ttl_seconds);
        let expires_at = record.expires_at;
        self.records.lock().await.insert(token_id.clone(), record);
        Ok(CredentialIssueResponse {
            token_id,
            token: token_value,
            expires_at,
        })
    }

    async fn validate(&self, token_id: &TokenId) -> Result<TokenRecord, CredentialError> {
        let records = self.records.lock().await;
        let record = records.get(token_id).ok_or_else(|| CredentialError::NotFound {
            message: "token not found".to_string(),
        })?;
        record.clone().usable(token_id, SystemTime::now())
    }

    async fn revoke(&self, token_id: &TokenId) -> Result<(), TokenStoreError> {
        if let Some(record) = self.records.lock().await.get_mut(token_id) {
            record.revoked = true;
        }
        Ok(())
    }
}

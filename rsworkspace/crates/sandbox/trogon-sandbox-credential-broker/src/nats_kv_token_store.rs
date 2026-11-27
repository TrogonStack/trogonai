use std::num::NonZeroU32;
use std::time::SystemTime;

use async_nats::jetstream::{self, kv};
use async_trait::async_trait;
use bytes::Bytes;
use trogon_sandbox_contract::{
    CredentialError, CredentialId, CredentialIssueResponse, CredentialScope, ShortLivedToken, TokenId,
};
use trogon_sandbox_protocol::subjects::CREDENTIAL_TOKEN_BUCKET;

use crate::token_record::new_token_id;
use crate::{BackendError, TokenRecord, TokenStore, TokenStoreError};

/// Tokens in a NATS KV bucket, keyed by token id.
pub struct NatsKvTokenStore {
    kv: kv::Store,
}

impl NatsKvTokenStore {
    pub fn new(kv: kv::Store) -> Self {
        Self { kv }
    }

    /// Creates the token bucket when it does not exist yet and opens it.
    pub async fn open_bucket(jetstream: &jetstream::Context) -> Result<kv::Store, TokenStoreError> {
        jetstream
            .create_or_update_key_value(kv::Config {
                bucket: CREDENTIAL_TOKEN_BUCKET.to_string(),
                history: 1,
                ..Default::default()
            })
            .await
            .map_err(|error| BackendError::new(error).into())
    }

    async fn put(&self, token_id: &TokenId, record: &TokenRecord) -> Result<(), TokenStoreError> {
        let payload = Bytes::from(serde_json::to_vec(record)?);
        self.kv
            .put(token_id.as_str(), payload)
            .await
            .map_err(BackendError::new)?;
        Ok(())
    }
}

#[async_trait]
impl TokenStore for NatsKvTokenStore {
    async fn issue(
        &self,
        credential_id: &CredentialId,
        scope: &CredentialScope,
        token_value: ShortLivedToken,
        ttl_seconds: NonZeroU32,
    ) -> Result<CredentialIssueResponse, TokenStoreError> {
        let token_id = new_token_id();
        let record = TokenRecord::new(credential_id, scope, token_value.clone(), ttl_seconds);
        self.put(&token_id, &record).await?;
        Ok(CredentialIssueResponse {
            token_id,
            token: token_value,
            expires_at: record.expires_at,
        })
    }

    async fn validate(&self, token_id: &TokenId) -> Result<TokenRecord, CredentialError> {
        let entry = self
            .kv
            .entry(token_id.as_str())
            .await
            .map_err(|error| CredentialError::Backend {
                message: error.to_string(),
            })?
            .ok_or_else(|| CredentialError::NotFound {
                message: "token not found".to_string(),
            })?;
        let record: TokenRecord = serde_json::from_slice(&entry.value).map_err(|error| CredentialError::Backend {
            message: error.to_string(),
        })?;
        record.usable(token_id, SystemTime::now())
    }

    async fn revoke(&self, token_id: &TokenId) -> Result<(), TokenStoreError> {
        match self.validate(token_id).await {
            Ok(mut record) => {
                record.revoked = true;
                self.put(token_id, &record).await
            }
            Err(
                CredentialError::TokenExpired { .. }
                | CredentialError::TokenRevoked { .. }
                | CredentialError::NotFound { .. },
            ) => Ok(()),
            Err(error) => Err(TokenStoreError::Rejected(error)),
        }
    }
}

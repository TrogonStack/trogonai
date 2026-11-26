use std::time::SystemTime;

use async_nats::jetstream::context::GetStreamErrorKind;
use async_nats::jetstream::{self, ErrorCode, kv};
use async_trait::async_trait;
use bytes::Bytes;
use rand_core::{OsRng, RngCore};
use trogon_sandbox_contract::{ServiceAccountId, TenantId};

use crate::{ApiKeyBucket, ApiKeyHash, ApiKeyRecord, ApiKeyStore, ApiKeyStoreError, BearerToken};

const TOKEN_BYTES: usize = 32;

/// An [`ApiKeyStore`] backed by a JetStream key-value bucket.
///
/// Records are stored as JSON under the key hash, so the bearer token itself is never persisted.
pub struct NatsKvApiKeyStore {
    kv: kv::Store,
}

impl NatsKvApiKeyStore {
    /// Opens the default bucket, creating it when it does not exist.
    pub async fn new(jetstream: jetstream::Context) -> Result<Self, ApiKeyStoreError> {
        Self::new_with_bucket(jetstream, &ApiKeyBucket::default()).await
    }

    /// Opens `bucket`, creating it with default settings only when it does not exist. An existing
    /// bucket keeps its configuration.
    pub async fn new_with_bucket(
        jetstream: jetstream::Context,
        bucket: &ApiKeyBucket,
    ) -> Result<Self, ApiKeyStoreError> {
        if bucket_exists(&jetstream, bucket).await? {
            let kv = jetstream
                .get_key_value(bucket.as_str())
                .await
                .map_err(ApiKeyStoreError::backend)?;
            return Ok(Self { kv });
        }
        let kv = jetstream
            .create_key_value(kv::Config {
                bucket: bucket.as_str().to_string(),
                ..Default::default()
            })
            .await
            .map_err(ApiKeyStoreError::backend)?;
        Ok(Self { kv })
    }

    pub fn kv(&self) -> &kv::Store {
        &self.kv
    }

    /// Generates a new random API key, stores its record, and returns the token. This is the only
    /// time the token is available; only its hash is stored.
    pub async fn mint(
        &self,
        tenant: TenantId,
        service_account: ServiceAccountId,
        name: impl Into<String>,
    ) -> Result<BearerToken, ApiKeyStoreError> {
        let mut raw = [0u8; TOKEN_BYTES];
        OsRng.try_fill_bytes(&mut raw).map_err(ApiKeyStoreError::Entropy)?;
        let token = BearerToken::new(hex::encode(raw)).map_err(ApiKeyStoreError::backend)?;
        let record = ApiKeyRecord::new(tenant, service_account, name, SystemTime::now());
        let bytes = Bytes::from(serde_json::to_vec(&record)?);
        self.kv
            .put(token.hash().as_str(), bytes)
            .await
            .map_err(ApiKeyStoreError::backend)?;
        Ok(token)
    }
}

async fn bucket_exists(jetstream: &jetstream::Context, bucket: &ApiKeyBucket) -> Result<bool, ApiKeyStoreError> {
    match jetstream.get_stream(format!("KV_{bucket}")).await {
        Ok(_) => Ok(true),
        Err(error)
            if matches!(error.kind(), GetStreamErrorKind::JetStream(ref source)
                if source.error_code() == ErrorCode::STREAM_NOT_FOUND) =>
        {
            Ok(false)
        }
        Err(error) => Err(ApiKeyStoreError::backend(error)),
    }
}

#[async_trait]
impl ApiKeyStore for NatsKvApiKeyStore {
    async fn lookup(&self, key_hash: &ApiKeyHash) -> Result<Option<ApiKeyRecord>, ApiKeyStoreError> {
        let Some(bytes) = self
            .kv
            .get(key_hash.as_str())
            .await
            .map_err(ApiKeyStoreError::backend)?
        else {
            return Ok(None);
        };
        Ok(Some(serde_json::from_slice(&bytes)?))
    }
}

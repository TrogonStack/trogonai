use async_nats::jetstream::{self, kv};
use async_trait::async_trait;
use trogon_sandbox_contract::TenantId;

use super::{NativePassthroughPolicy, TenantNativePassthroughPolicy};
use crate::tenant_record::{self, TenantRecordError};

/// Per-tenant records in a NATS KV bucket, keyed like the other tenant
/// allow-lists.
pub struct NatsKvNativePassthroughStore {
    pub kv: kv::Store,
}

impl NatsKvNativePassthroughStore {
    pub async fn new(jetstream: jetstream::Context, bucket: &str) -> Result<Self, TenantRecordError> {
        Ok(Self {
            kv: tenant_record::open_bucket(&jetstream, bucket).await?,
        })
    }

    /// The stored record of `tenant`, if any.
    pub async fn policy_for(
        &self,
        tenant: &TenantId,
    ) -> Result<Option<TenantNativePassthroughPolicy>, TenantRecordError> {
        tenant_record::get_parsed(&self.kv, tenant).await
    }
}

#[async_trait]
impl NativePassthroughPolicy for NatsKvNativePassthroughStore {
    async fn allows(&self, tenant: &TenantId) -> Result<bool, TenantRecordError> {
        Ok(self.policy_for(tenant).await?.is_some_and(|policy| policy.allowed))
    }
}

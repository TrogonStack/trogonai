use async_nats::jetstream::{self, kv};
use async_trait::async_trait;
use trogon_sandbox_contract::TenantId;

use super::{ImageAllowList, TenantImagePolicy};
use crate::tenant_record::{self, TenantRecordError};

pub struct NatsKvImageAllowList {
    pub kv: kv::Store,
}

impl NatsKvImageAllowList {
    pub async fn new(jetstream: jetstream::Context, bucket: &str) -> Result<Self, TenantRecordError> {
        Ok(Self {
            kv: tenant_record::open_bucket(&jetstream, bucket).await?,
        })
    }
}

#[async_trait]
impl ImageAllowList for NatsKvImageAllowList {
    async fn allowed_for(&self, tenant: &TenantId) -> Result<Option<TenantImagePolicy>, TenantRecordError> {
        tenant_record::get_parsed(&self.kv, tenant).await
    }
}

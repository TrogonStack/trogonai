use async_nats::jetstream::{self, kv};
use async_trait::async_trait;
use trogon_sandbox_contract::TenantId;

use super::{TenantLimit, TenantLimits};
use crate::tenant_record::{self, TenantRecordError};

pub struct NatsKvTenantLimits {
    pub kv: kv::Store,
}

impl NatsKvTenantLimits {
    pub async fn new(jetstream: jetstream::Context, bucket: &str) -> Result<Self, TenantRecordError> {
        Ok(Self {
            kv: tenant_record::open_bucket(&jetstream, bucket).await?,
        })
    }
}

#[async_trait]
impl TenantLimits for NatsKvTenantLimits {
    async fn for_tenant(&self, tenant: &TenantId) -> Result<Option<TenantLimit>, TenantRecordError> {
        tenant_record::get_parsed(&self.kv, tenant).await
    }
}

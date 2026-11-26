use std::time::SystemTime;

use serde::{Deserialize, Serialize};
use trogon_sandbox_contract::{ServiceAccountId, TenantId};

use crate::api_key_record_wire::ApiKeyRecordWire;

/// What an API key authenticates as: the tenant and service account it acts for.
///
/// It serializes as a JSON object with `tenant`, `service_account`, `name`, and `created_at` as
/// whole seconds since the Unix epoch. Deserialization enforces the tenant identifier rules.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "ApiKeyRecordWire", into = "ApiKeyRecordWire")]
pub struct ApiKeyRecord {
    tenant: TenantId,
    service_account: ServiceAccountId,
    name: String,
    created_at: SystemTime,
}

impl ApiKeyRecord {
    pub fn new(
        tenant: TenantId,
        service_account: ServiceAccountId,
        name: impl Into<String>,
        created_at: SystemTime,
    ) -> Self {
        Self {
            tenant,
            service_account,
            name: name.into(),
            created_at,
        }
    }

    pub fn tenant(&self) -> &TenantId {
        &self.tenant
    }

    pub fn service_account(&self) -> &ServiceAccountId {
        &self.service_account
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn created_at(&self) -> SystemTime {
        self.created_at
    }
}

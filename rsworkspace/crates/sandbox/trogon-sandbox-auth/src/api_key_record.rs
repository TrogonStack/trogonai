use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use trogon_sandbox_contract::{ServiceAccountId, TenantId};

use crate::ApiKeyRecordError;

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

/// The stored JSON shape of an API key record; it is parsed into [`ApiKeyRecord`] before use.
#[derive(Serialize, Deserialize)]
pub(crate) struct ApiKeyRecordWire {
    tenant: String,
    service_account: String,
    name: String,
    created_at: u64,
}

impl TryFrom<ApiKeyRecordWire> for ApiKeyRecord {
    type Error = ApiKeyRecordError;

    fn try_from(wire: ApiKeyRecordWire) -> Result<Self, Self::Error> {
        let created_at = UNIX_EPOCH
            .checked_add(Duration::from_secs(wire.created_at))
            .ok_or(ApiKeyRecordError::CreatedAtOutOfRange)?;
        Ok(Self::new(
            TenantId::new(wire.tenant)?,
            ServiceAccountId::new(wire.service_account),
            wire.name,
            created_at,
        ))
    }
}

impl From<ApiKeyRecord> for ApiKeyRecordWire {
    fn from(record: ApiKeyRecord) -> Self {
        let created_at = record
            .created_at()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_secs());
        Self {
            tenant: record.tenant().to_string(),
            service_account: record.service_account().to_string(),
            name: record.name().to_string(),
            created_at,
        }
    }
}

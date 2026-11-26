use std::time::{Duration, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use trogon_sandbox_contract::{ServiceAccountId, TenantId};

use crate::{ApiKeyRecord, ApiKeyRecordError};

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

use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use trogon_sandbox_contract::{ServiceAccountId, TenantId};

/// What a session token resolves to.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionRecord {
    pub tenant: TenantId,
    pub service_account: ServiceAccountId,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub api_key: String,
    pub expires_at: u64,
}

impl SessionRecord {
    pub fn is_expired(&self) -> bool {
        now_secs() >= self.expires_at
    }
}

/// Seconds since the Unix epoch, or zero when the clock reads before it.
pub(crate) fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs())
}

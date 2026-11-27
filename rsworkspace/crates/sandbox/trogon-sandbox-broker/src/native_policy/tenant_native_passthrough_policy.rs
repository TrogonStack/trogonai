use serde::{Deserialize, Serialize};

/// Stored per-tenant record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TenantNativePassthroughPolicy {
    pub allowed: bool,
}

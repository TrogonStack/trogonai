use serde::{Deserialize, Serialize};

use crate::{LeaseId, TenantId};

/// Client-facing request to fork a currently running lease into a brand-new, independent lease seeded from its live state.
///
/// The source lease keeps running untouched. Only the new lease is subject to `ttl_seconds` and `idempotency_key`.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct LeaseForkRequest {
    pub tenant: TenantId,
    pub source_lease_id: LeaseId,
    pub ttl_seconds: u32,
    pub idempotency_key: String,
}

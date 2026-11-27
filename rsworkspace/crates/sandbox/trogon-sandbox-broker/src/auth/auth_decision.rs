use trogon_sandbox_contract::{ServiceAccountId, TenantId};

use super::AuthDenialReason;

// The resolved identity is kept for callers that need it; the broker's current
// call sites only match on the variant.
#[allow(dead_code)]
pub enum AuthDecision {
    Allowed {
        tenant: TenantId,
        principal: ServiceAccountId,
    },
    Denied(AuthDenialReason),
}

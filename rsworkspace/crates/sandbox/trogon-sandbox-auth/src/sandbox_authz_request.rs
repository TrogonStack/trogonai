use trogon_sandbox_contract::{ServiceAccountId, TenantId};

/// Asks whether a service account may lease sandboxes for a tenant.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct SandboxAuthzRequest {
    pub tenant: TenantId,
    pub principal: ServiceAccountId,
}

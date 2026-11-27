use trogon_sandbox_contract::{OperationId, TenantId};

/// Proof of a claim: only writes carrying the generation it names succeed.
#[derive(Debug, Clone)]
pub struct ClaimToken {
    pub tenant: TenantId,
    pub id: OperationId,
    pub generation: u64,
}

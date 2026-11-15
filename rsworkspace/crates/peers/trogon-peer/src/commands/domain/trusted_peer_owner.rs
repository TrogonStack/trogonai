use super::{CallerId, TenantId};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrustedPeerOwner {
    tenant: TenantId,
    caller: CallerId,
}

impl TrustedPeerOwner {
    pub fn new(tenant: TenantId, caller: CallerId) -> Self {
        Self { tenant, caller }
    }

    pub fn tenant(&self) -> &TenantId {
        &self.tenant
    }

    pub fn caller(&self) -> &CallerId {
        &self.caller
    }
}

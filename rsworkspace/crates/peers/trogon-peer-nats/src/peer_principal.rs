use crate::{PeerCaller, PeerTenant};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeerPrincipal {
    tenant: PeerTenant,
    caller: PeerCaller,
}

impl PeerPrincipal {
    pub fn new(tenant: PeerTenant, caller: PeerCaller) -> Self {
        Self { tenant, caller }
    }

    pub fn tenant(&self) -> &PeerTenant {
        &self.tenant
    }

    pub fn caller(&self) -> &PeerCaller {
        &self.caller
    }
}

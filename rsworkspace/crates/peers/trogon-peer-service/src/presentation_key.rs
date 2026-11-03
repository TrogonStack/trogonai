use trogon_peer::{BatchId, PeerId, TenantId};
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PresentationKey {
    tenant: TenantId,
    peer: PeerId,
    batch: BatchId,
}
impl PresentationKey {
    pub fn new(tenant: TenantId, peer: PeerId, batch: BatchId) -> Self {
        Self { tenant, peer, batch }
    }
    pub fn tenant(&self) -> &TenantId {
        &self.tenant
    }
    pub fn peer(&self) -> &PeerId {
        &self.peer
    }
    pub fn batch(&self) -> &BatchId {
        &self.batch
    }
}

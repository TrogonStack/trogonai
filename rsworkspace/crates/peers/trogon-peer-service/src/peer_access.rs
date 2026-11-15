use crate::peer_operation::PeerOperation;
use trogon_peer::{PeerId, TrustedPeerOwner};
#[derive(Debug, Clone)]
pub struct PeerAccess {
    owner: TrustedPeerOwner,
    peer: PeerId,
    operation: PeerOperation,
}
impl PeerAccess {
    pub fn new(owner: TrustedPeerOwner, peer: PeerId, operation: PeerOperation) -> Self {
        Self { owner, peer, operation }
    }
    pub fn owner(&self) -> &TrustedPeerOwner {
        &self.owner
    }
    pub fn peer(&self) -> &PeerId {
        &self.peer
    }
    pub fn operation(&self) -> PeerOperation {
        self.operation
    }
}

use super::{ConnectorId, PeerId, PeerValueError};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttachmentRef {
    peer_id: PeerId,
    connector_id: ConnectorId,
    generation: u64,
}

impl AttachmentRef {
    pub fn new(peer_id: PeerId, connector_id: ConnectorId, generation: u64) -> Result<Self, PeerValueError> {
        Ok(Self {
            peer_id,
            connector_id,
            generation,
        })
    }
    pub fn peer_id(&self) -> &PeerId {
        &self.peer_id
    }
    pub fn connector_id(&self) -> &ConnectorId {
        &self.connector_id
    }
    pub fn generation(&self) -> u64 {
        self.generation
    }
}

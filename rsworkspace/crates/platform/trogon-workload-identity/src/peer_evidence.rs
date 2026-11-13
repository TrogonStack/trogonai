use std::time::SystemTime;

use tokio::time::Instant;

use crate::ApplicationId;
use spiffe::SpiffeId;

#[derive(Debug, Clone)]
pub struct PeerEvidence {
    pub(crate) application_id: ApplicationId,
    pub(crate) expires_at: SystemTime,
    pub(crate) deadline: Instant,
    pub(crate) spiffe_id: SpiffeId,
    pub(crate) leaf_sha256: [u8; 32],
}

impl PeerEvidence {
    pub fn application_id(&self) -> &ApplicationId {
        &self.application_id
    }
    pub fn expires_at(&self) -> SystemTime {
        self.expires_at
    }
    pub fn deadline(&self) -> Instant {
        self.deadline
    }
    pub fn spiffe_id(&self) -> &SpiffeId {
        &self.spiffe_id
    }
    pub fn leaf_sha256(&self) -> &[u8; 32] {
        &self.leaf_sha256
    }
}

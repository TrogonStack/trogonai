use std::time::SystemTime;

use crate::ApplicationId;

#[derive(Debug, Clone)]
pub struct PeerEvidence {
    pub(crate) application_id: ApplicationId,
    pub(crate) expires_at: SystemTime,
}

impl PeerEvidence {
    pub fn application_id(&self) -> &ApplicationId { &self.application_id }
    pub fn expires_at(&self) -> SystemTime { self.expires_at }
}

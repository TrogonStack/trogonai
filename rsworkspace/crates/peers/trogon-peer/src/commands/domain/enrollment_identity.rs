use super::{InstallationId, NativeSessionId, PeerId, TrustedPeerOwner};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnrollmentIdentity {
    owner: TrustedPeerOwner,
    installation: InstallationId,
    native_session: NativeSessionId,
    peer_id: PeerId,
}

impl EnrollmentIdentity {
    pub fn new(owner: TrustedPeerOwner, installation: InstallationId, native_session: NativeSessionId) -> Self {
        let peer_id = PeerId::derive(&owner, &installation, &native_session);
        Self {
            owner,
            installation,
            native_session,
            peer_id,
        }
    }

    pub fn owner(&self) -> &TrustedPeerOwner {
        &self.owner
    }

    pub fn installation(&self) -> &InstallationId {
        &self.installation
    }

    pub fn native_session(&self) -> &NativeSessionId {
        &self.native_session
    }

    pub fn peer_id(&self) -> &PeerId {
        &self.peer_id
    }
}

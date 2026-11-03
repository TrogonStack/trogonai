use crate::{PeerAccount, PeerConnectionId, PeerIngressError, PeerPrincipal};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeerPermissions {
    publish_allow: Vec<String>,
    subscribe_allow: Vec<String>,
}

impl PeerPermissions {
    pub fn for_connection(
        _account: &PeerAccount,
        _principal: &PeerPrincipal,
        _connection: &PeerConnectionId,
    ) -> Result<Self, PeerIngressError> {
        Ok(Self {
            publish_allow: Vec::new(),
            subscribe_allow: Vec::new(),
        })
    }

    pub fn publish_allow(&self) -> &[String] {
        &self.publish_allow
    }

    pub fn subscribe_allow(&self) -> &[String] {
        &self.subscribe_allow
    }
}

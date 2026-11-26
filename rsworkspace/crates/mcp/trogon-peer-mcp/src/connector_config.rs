use trogon_peer::{CollaborationRef, ConnectorId, InstallationId, PeerDisplayName, PeerValueError};

use crate::ConnectorConfigInput;

pub(crate) struct ConnectorConfig {
    installation_id: InstallationId,
    connector_id: ConnectorId,
    collaboration_ref: CollaborationRef,
    display_name: PeerDisplayName,
}

impl TryFrom<ConnectorConfigInput> for ConnectorConfig {
    type Error = PeerValueError;

    fn try_from(input: ConnectorConfigInput) -> Result<Self, Self::Error> {
        Ok(Self {
            installation_id: InstallationId::new(input.installation_id)?,
            connector_id: ConnectorId::new(input.connector_id)?,
            collaboration_ref: CollaborationRef::new(input.collaboration_ref)?,
            display_name: PeerDisplayName::new(input.display_name)?,
        })
    }
}

impl ConnectorConfig {
    pub(crate) fn installation_id(&self) -> &InstallationId {
        &self.installation_id
    }

    pub(crate) fn connector_id(&self) -> &ConnectorId {
        &self.connector_id
    }

    pub(crate) fn collaboration_ref(&self) -> &CollaborationRef {
        &self.collaboration_ref
    }

    pub(crate) fn display_name(&self) -> &PeerDisplayName {
        &self.display_name
    }
}

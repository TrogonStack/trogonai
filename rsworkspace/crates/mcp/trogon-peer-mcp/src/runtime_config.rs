use std::fmt;

use trogon_nats::NatsConfig;
use trogon_peer_nats::PeerRequesterConfig;
use trogon_std::env::ReadEnv;

use crate::availability_observation::AvailabilityObservation;
use crate::connector_config::ConnectorConfig;
use crate::connector_config_input::ConnectorConfigInput;
use crate::runtime_config_error::RuntimeConfigError;

pub struct RuntimeConfig {
    requester: PeerRequesterConfig,
    connector: ConnectorConfig,
    nats: NatsConfig,
    availability: AvailabilityObservation,
}

impl fmt::Debug for RuntimeConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_struct("RuntimeConfig").finish_non_exhaustive()
    }
}

impl RuntimeConfig {
    pub fn from_env<E: ReadEnv>(_env: &E) -> Result<Self, RuntimeConfigError> {
        Err(RuntimeConfigError::Missing("PEER_TENANT_ID"))
    }

    pub fn requester(&self) -> &PeerRequesterConfig {
        &self.requester
    }
    pub fn nats(&self) -> &NatsConfig {
        &self.nats
    }
    pub fn availability(&self) -> AvailabilityObservation {
        self.availability
    }
    pub fn connector_input(&self) -> ConnectorConfigInput {
        ConnectorConfigInput {
            installation_id: self.connector.installation_id().as_str().to_owned(),
            connector_id: self.connector.connector_id().as_str().to_owned(),
            collaboration_ref: self.connector.collaboration_ref().as_str().to_owned(),
            display_name: self.connector.display_name().as_str().to_owned(),
        }
    }
}

use std::fmt;

use std::time::Duration;

use trogon_nats::{NatsAuth, NatsConfig};
use trogon_peer::{CollaborationRef, ConnectorId, InstallationId, PeerDisplayName};
use trogon_peer_nats::{
    PeerAccount, PeerCaller, PeerConnectionId, PeerRequestTimeout, PeerRequesterConfig, PeerTenant,
};
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
    pub fn from_env<E: ReadEnv>(env: &E) -> Result<Self, RuntimeConfigError> {
        let tenant = PeerTenant::new(required(env, "PEER_TENANT_ID")?).map_err(RuntimeConfigError::Routing)?;
        let account = match required(env, "PEER_ACCOUNT_MODE")?.as_str() {
            "tenant" => PeerAccount::TenantAccount { tenant },
            "shared_fixture" => PeerAccount::SharedFixture { tenant },
            _ => return Err(RuntimeConfigError::Mode),
        };
        let caller = PeerCaller::new(required(env, "PEER_CALLER_ID")?).map_err(RuntimeConfigError::Routing)?;
        let connection =
            PeerConnectionId::new(required(env, "PEER_CONNECTION_ID")?).map_err(RuntimeConfigError::Routing)?;
        let connector = ConnectorConfig::from_parts(
            InstallationId::new(required(env, "PEER_INSTALLATION_ID")?).map_err(RuntimeConfigError::Identity)?,
            ConnectorId::new(uuid::Uuid::new_v4().to_string()).map_err(RuntimeConfigError::Identity)?,
            CollaborationRef::new(required(env, "PEER_COLLABORATION_REF")?).map_err(RuntimeConfigError::Identity)?,
            PeerDisplayName::new(required(env, "PEER_DISPLAY_NAME")?).map_err(RuntimeConfigError::Identity)?,
        );
        let availability = match required(env, "PEER_AVAILABILITY")?.as_str() {
            "ready" => AvailabilityObservation::Ready,
            "busy" => AvailabilityObservation::Busy,
            "blocked" => AvailabilityObservation::Blocked,
            "draining" => AvailabilityObservation::Draining,
            _ => return Err(RuntimeConfigError::Availability),
        };
        let nats = NatsConfig::from_env(env);
        if matches!(nats.auth, NatsAuth::None) {
            return Err(RuntimeConfigError::Anonymous);
        }
        let timeout = PeerRequestTimeout::new(Duration::from_secs(2)).map_err(RuntimeConfigError::Timeout)?;
        Ok(Self {
            requester: PeerRequesterConfig::new(account, caller, connection, timeout),
            connector,
            nats,
            availability,
        })
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

fn required<E: ReadEnv>(env: &E, setting: &'static str) -> Result<String, RuntimeConfigError> {
    env.var(setting)
        .ok()
        .filter(|value| !value.is_empty())
        .ok_or(RuntimeConfigError::Missing(setting))
}

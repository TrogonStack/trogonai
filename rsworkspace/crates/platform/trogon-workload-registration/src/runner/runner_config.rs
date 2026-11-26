use ed25519_dalek::VerifyingKey;
use trogon_nats::constants::ENV_NATS_URL;
use trogon_nats::{NatsAuth, NatsConfig, NatsTls};
use trogon_std::ReadEnv;
use trogon_workload_registration::ProducerConfig;

use super::configuration_wire::ConfigurationWire;
use super::runner_error::RunnerError;

pub(super) struct RunnerConfig {
    producer: ProducerConfig,
    nats: NatsConfig,
}

impl RunnerConfig {
    pub fn from_wire(wire: ConfigurationWire, env: &impl ReadEnv) -> Result<Self, RunnerError> {
        let mut bytes = [0u8; 32];
        hex::decode_to_slice(&wire.approval_public_key, &mut bytes).map_err(RunnerError::PublicKeyEncoding)?;
        let key = VerifyingKey::from_bytes(&bytes).map_err(RunnerError::PublicKey)?;
        let producer = ProducerConfig::new(
            &wire.tenant,
            &wire.project,
            &wire.trust_domain,
            wire.recovery_fence,
            key,
        )
        .map_err(RunnerError::configuration)?;
        let explicit_url = env.var(ENV_NATS_URL).map_err(RunnerError::Environment)?;
        if explicit_url.trim().is_empty() {
            return Err(RunnerError::ExplicitNats);
        }
        let nats = NatsConfig::from_env(env);
        if nats.servers.is_empty() {
            return Err(RunnerError::ExplicitNats);
        }
        let client_certificate = match &nats.tls {
            Ok(NatsTls::Enabled { client_cert, .. }) => client_cert.is_some(),
            _ => return Err(RunnerError::Transport),
        };
        match &nats.auth {
            NatsAuth::Jwt { seed: None, .. } => return Err(RunnerError::Authentication),
            NatsAuth::None if !client_certificate => return Err(RunnerError::Authentication),
            _ => {}
        }
        Ok(Self { producer, nats })
    }

    pub fn producer(&self) -> &ProducerConfig {
        &self.producer
    }

    pub fn nats(&self) -> &NatsConfig {
        &self.nats
    }
}

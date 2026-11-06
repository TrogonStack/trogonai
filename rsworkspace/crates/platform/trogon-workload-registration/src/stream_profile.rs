use async_nats::jetstream::stream::{Compression, Config, DiscardPolicy, StorageType};
use trogon_nats::{DottedNatsToken, NatsToken};
use trogon_workload_identity::constants::MAX_REGISTRATION_LIFETIME;

use crate::constants::{MAX_STREAM_BYTES, STREAM_DUPLICATE_WINDOW};
use crate::{MAX_SNAPSHOT_BYTES, ProducerConfig, RegistrationError, RegistrationRole, Sha256Digest};

#[derive(Debug, Clone)]
pub struct StreamProfile {
    config: ProducerConfig,
    control: String,
    live: String,
}

impl StreamProfile {
    pub fn new(config: &ProducerConfig) -> Self {
        let scope = format!(
            "{}:{}:{}",
            config.scope().trust_domain(),
            config.scope().tenant().as_str(),
            config.scope().project()
        );
        let fingerprint: String = Sha256Digest::hash(scope.as_bytes()).to_hex().chars().take(32).collect();
        let fence = config.recovery_fence();
        Self {
            config: config.clone(),
            control: format!("wr_control_{fingerprint}_{fence}"),
            live: format!("wr_live_{fingerprint}_{fence}"),
        }
    }

    pub fn control_bucket(&self) -> &str {
        &self.control
    }
    pub fn live_bucket(&self) -> &str {
        &self.live
    }
    pub fn control_subject(&self) -> String {
        format!("$KV.{}.{}", self.control, self.config.scope().key())
    }
    pub fn live_subject(&self) -> String {
        format!("$KV.{}.{}", self.live, self.config.scope().key())
    }

    pub fn inbox_prefix(&self, role: RegistrationRole) -> Result<DottedNatsToken, RegistrationError> {
        DottedNatsToken::new(format!("_INBOX.workload_registration.{}.{}", self.live, role.as_str()))
            .map_err(RegistrationError::Project)
    }

    pub fn audit_component(&self, role: RegistrationRole) -> Result<NatsToken, RegistrationError> {
        NatsToken::new(format!("{}.{}", self.live, role.as_str()).replace('.', "_")).map_err(RegistrationError::Project)
    }

    pub fn publish_allowlist(&self, role: RegistrationRole) -> Vec<String> {
        let mut subjects = Vec::new();
        for stream in [self.control_config().name, self.live_config().name] {
            subjects.push(format!("$JS.API.STREAM.INFO.{stream}"));
            if role == RegistrationRole::Provisioner {
                subjects.push(format!("$JS.API.STREAM.CREATE.{stream}"));
            } else {
                subjects.push(format!("$JS.API.STREAM.MSG.GET.{stream}"));
            }
        }
        if role != RegistrationRole::Provisioner {
            subjects.extend([self.control_subject(), self.live_subject()]);
            subjects.push(format!(
                "platform.v1.audit.change.{}.policy",
                self.config.scope().project()
            ));
            subjects.push(format!(
                "platform.v1.audit.integrity.{}_{}.chain_reset",
                self.live,
                role.as_str()
            ));
        }
        subjects
    }

    pub fn subscribe_allowlist(&self, role: RegistrationRole) -> Vec<String> {
        vec![format!(
            "_INBOX.workload_registration.{}.{}.>",
            self.live,
            role.as_str()
        )]
    }

    pub fn control_config(&self) -> Config {
        Config {
            name: format!("KV_{}", self.control),
            subjects: vec![self.control_subject()],
            num_replicas: 3,
            max_messages: 1,
            max_messages_per_subject: 1,
            max_message_size: MAX_SNAPSHOT_BYTES as i32,
            max_bytes: MAX_STREAM_BYTES,
            max_consumers: -1,
            duplicate_window: STREAM_DUPLICATE_WINDOW,
            storage: StorageType::File,
            compression: Some(Compression::None),
            discard: DiscardPolicy::Old,
            deny_delete: true,
            deny_purge: true,
            ..Config::default()
        }
    }

    pub fn live_config(&self) -> Config {
        Config {
            name: format!("KV_{}", self.live),
            subjects: vec![self.live_subject()],
            max_age: MAX_REGISTRATION_LIFETIME,
            ..self.control_config()
        }
    }

    pub(crate) fn validate(&self, actual: &Config, live: bool) -> Result<(), RegistrationError> {
        let expected = if live {
            self.live_config()
        } else {
            self.control_config()
        };
        let mut normalized = actual.clone();
        // The server adds advisory version metadata. It conveys no authority.
        normalized.metadata = expected.metadata.clone();
        if normalized != expected {
            return Err(RegistrationError::StreamProfile);
        }
        Ok(())
    }
}

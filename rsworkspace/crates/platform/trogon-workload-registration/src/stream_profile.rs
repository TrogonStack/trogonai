use async_nats::jetstream::stream::{Config, DiscardPolicy, StorageType};
use trogon_workload_identity::constants::MAX_REGISTRATION_LIFETIME;

use crate::{MAX_SNAPSHOT_BYTES, ProducerConfig, Sha256Digest};

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

    pub fn control_config(&self) -> Config {
        Config {
            name: format!("KV_{}", self.control),
            subjects: vec![self.control_subject()],
            num_replicas: 3,
            max_messages: 1,
            max_messages_per_subject: 1,
            max_message_size: MAX_SNAPSHOT_BYTES as i32,
            max_bytes: 1024 * 1024,
            storage: StorageType::File,
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
}

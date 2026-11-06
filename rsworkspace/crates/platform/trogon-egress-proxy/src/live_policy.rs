use std::num::NonZeroU64;
use std::sync::Arc;
use std::time::SystemTime;

use serde::Deserialize;
use tokio_util::sync::CancellationToken;
use trogon_audit::AuditName;
use trogon_nats::NatsToken;

use crate::agent_id::AgentId;
use crate::destination::Destination;
use crate::policy_snapshot::{PolicySnapshot, SnapshotVerdict};
use crate::policy_source::PolicySource;
use crate::static_snapshot::{SnapshotError, StaticSnapshot};

#[derive(Debug, Clone)]
pub struct KvPolicyConfig {
    pub bucket: String,
    pub tenant: AuditName,
    pub project: NatsToken,
    pub incarnation: NonZeroU64,
}

#[derive(Debug, thiserror::Error)]
pub enum LivePolicyError {
    #[error("policy bucket is unavailable")]
    Bucket(#[source] async_nats::jetstream::context::KeyValueError),
    #[error("policy entry is unavailable")]
    Entry(#[source] async_nats::jetstream::kv::EntryError),
    #[error("policy entry is absent")]
    Missing,
    #[error("policy envelope is invalid")]
    Envelope(#[source] serde_json::Error),
    #[error("policy content is invalid")]
    Policy(#[source] SnapshotError),
}

pub struct KvSnapshot {
    snapshot: StaticSnapshot,
}

pub struct KvPolicyDriver;

#[derive(Deserialize)]
struct Envelope {
    policy: String,
}

impl KvSnapshot {
    pub async fn open(
        client: async_nats::Client,
        config: KvPolicyConfig,
    ) -> Result<(Arc<Self>, KvPolicyDriver), LivePolicyError> {
        let store = async_nats::jetstream::new(client)
            .get_key_value(&config.bucket)
            .await
            .map_err(LivePolicyError::Bucket)?;
        let entry = store
            .entry("current")
            .await
            .map_err(LivePolicyError::Entry)?
            .ok_or(LivePolicyError::Missing)?;
        let envelope: Envelope = serde_json::from_slice(&entry.value).map_err(LivePolicyError::Envelope)?;
        let snapshot = StaticSnapshot::from_toml(
            PolicySource::NatsKv {
                bucket: config.bucket,
                incarnation: config.incarnation,
            },
            &envelope.policy,
        )
        .map_err(LivePolicyError::Policy)?;
        Ok((Arc::new(Self { snapshot }), KvPolicyDriver))
    }
}

impl PolicySnapshot for KvSnapshot {
    fn source(&self) -> &PolicySource {
        self.snapshot.source()
    }

    fn lookup(&self, agent: &AgentId, destination: &Destination, now: SystemTime) -> SnapshotVerdict {
        self.snapshot.lookup(agent, destination, now)
    }
}

impl KvPolicyDriver {
    pub async fn run(self, shutdown: CancellationToken) {
        shutdown.cancelled().await;
    }
}

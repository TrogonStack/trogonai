//! Person Server backing store. In-memory by default; a JetStream KV-backed
//! implementation is provided for HA deployments (see `aauth-design.md §D6/§D9`).

use std::collections::HashMap;
use std::sync::Mutex;

use async_nats::jetstream::{self, kv};
use async_trait::async_trait;
use bytes::Bytes;
use serde::{Deserialize, Serialize};
use trogon_aauth_verify::ReplayStore;
use trogon_aauth_verify::replay::ReplayError;
use trogon_nats::jetstream::is_create_key_value_already_exists;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentRecord {
    pub agent_id: String,
    pub cnf_jwk: serde_json::Value,
    pub principal: Option<String>,
    pub iat: i64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConsentRecord {
    pub principal: String,
    pub agent_id: String,
    pub resource_iss: String,
    pub scope: String,
    pub exp: i64,
}

#[async_trait]
pub trait PersonStore: Send + Sync {
    async fn put_agent(&self, agent: AgentRecord) -> Result<(), StoreError>;
    async fn get_agent(&self, agent_id: &str) -> Result<Option<AgentRecord>, StoreError>;
    async fn put_consent(&self, consent: ConsentRecord) -> Result<(), StoreError>;
    async fn get_consent(
        &self,
        principal: &str,
        agent_id: &str,
        resource_iss: &str,
    ) -> Result<Option<ConsentRecord>, StoreError>;
}

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("backend: {0}")]
    Backend(String),
    #[error("kv put: {0}")]
    KvPut(#[source] kv::PutError),
    #[error("kv get: {0}")]
    KvGet(#[source] kv::EntryError),
    #[error("bucket open: {0}")]
    BucketOpen(#[source] jetstream::context::KeyValueError),
    #[error("bucket create: {0}")]
    BucketCreate(#[source] jetstream::context::CreateKeyValueError),
    #[error("serialize: {0}")]
    Serialize(#[source] serde_json::Error),
    #[error("deserialize: {0}")]
    Deserialize(#[source] serde_json::Error),
}

pub struct InMemoryStore {
    agents: Mutex<HashMap<String, AgentRecord>>,
    consents: Mutex<HashMap<String, ConsentRecord>>,
}

impl Default for InMemoryStore {
    fn default() -> Self {
        Self {
            agents: Mutex::new(HashMap::new()),
            consents: Mutex::new(HashMap::new()),
        }
    }
}

fn consent_key(principal: &str, agent_id: &str, resource_iss: &str) -> String {
    format!("{principal}|{agent_id}|{resource_iss}")
}

#[async_trait]
impl PersonStore for InMemoryStore {
    async fn put_agent(&self, agent: AgentRecord) -> Result<(), StoreError> {
        self.agents
            .lock()
            .map_err(|e| StoreError::Backend(e.to_string()))?
            .insert(agent.agent_id.clone(), agent);
        Ok(())
    }

    async fn get_agent(&self, agent_id: &str) -> Result<Option<AgentRecord>, StoreError> {
        Ok(self
            .agents
            .lock()
            .map_err(|e| StoreError::Backend(e.to_string()))?
            .get(agent_id)
            .cloned())
    }

    async fn put_consent(&self, consent: ConsentRecord) -> Result<(), StoreError> {
        let key = consent_key(&consent.principal, &consent.agent_id, &consent.resource_iss);
        self.consents
            .lock()
            .map_err(|e| StoreError::Backend(e.to_string()))?
            .insert(key, consent);
        Ok(())
    }

    async fn get_consent(
        &self,
        principal: &str,
        agent_id: &str,
        resource_iss: &str,
    ) -> Result<Option<ConsentRecord>, StoreError> {
        let key = consent_key(principal, agent_id, resource_iss);
        Ok(self
            .consents
            .lock()
            .map_err(|e| StoreError::Backend(e.to_string()))?
            .get(&key)
            .cloned())
    }
}

pub const AGENTS_BUCKET: &str = "aauth-agents";
pub const CONSENTS_BUCKET: &str = "aauth-consents";
pub const REPLAY_BUCKET: &str = "aauth-replay";

const AGENT_KEY_MAX_VALUE_SIZE: i32 = 32_768;
const CONSENT_KEY_MAX_VALUE_SIZE: i32 = 8_192;
const REPLAY_KEY_MAX_VALUE_SIZE: i32 = 64;

#[must_use]
pub fn agents_bucket_config() -> kv::Config {
    kv::Config {
        bucket: AGENTS_BUCKET.to_owned(),
        history: 1,
        max_value_size: AGENT_KEY_MAX_VALUE_SIZE,
        ..Default::default()
    }
}

#[must_use]
pub fn consents_bucket_config() -> kv::Config {
    kv::Config {
        bucket: CONSENTS_BUCKET.to_owned(),
        history: 1,
        max_value_size: CONSENT_KEY_MAX_VALUE_SIZE,
        ..Default::default()
    }
}

/// JetStream KV–backed implementation of [`PersonStore`] for HA deployments.
///
/// Layout (per `docs/identity/aauth-design.md §D6`):
/// - agents bucket keyed by `agent_id`,
/// - consents bucket keyed by `principal|agent_id|resource_iss`.
#[derive(Clone)]
pub struct JetStreamStore {
    agents: kv::Store,
    consents: kv::Store,
}

impl JetStreamStore {
    #[must_use]
    pub fn new(agents: kv::Store, consents: kv::Store) -> Self {
        Self { agents, consents }
    }

    /// Open (and optionally create) both buckets on the given JetStream context.
    pub async fn open(js: &jetstream::Context, auto_create: bool) -> Result<Self, StoreError> {
        let agents = open_or_create(js, agents_bucket_config(), AGENTS_BUCKET, auto_create).await?;
        let consents = open_or_create(js, consents_bucket_config(), CONSENTS_BUCKET, auto_create).await?;
        Ok(Self::new(agents, consents))
    }
}

async fn open_or_create(
    js: &jetstream::Context,
    config: kv::Config,
    name: &str,
    auto_create: bool,
) -> Result<kv::Store, StoreError> {
    if auto_create {
        match js.create_key_value(config).await {
            Ok(store) => Ok(store),
            Err(error) if is_create_key_value_already_exists(&error) => js
                .get_key_value(name.to_owned())
                .await
                .map_err(StoreError::BucketOpen),
            Err(error) => Err(StoreError::BucketCreate(error)),
        }
    } else {
        js.get_key_value(name.to_owned())
            .await
            .map_err(StoreError::BucketOpen)
    }
}

#[async_trait]
impl PersonStore for JetStreamStore {
    async fn put_agent(&self, agent: AgentRecord) -> Result<(), StoreError> {
        let key = agent.agent_id.clone();
        let bytes = serde_json::to_vec(&agent).map_err(StoreError::Serialize)?;
        self.agents.put(key, bytes.into()).await.map_err(StoreError::KvPut)?;
        Ok(())
    }

    async fn get_agent(&self, agent_id: &str) -> Result<Option<AgentRecord>, StoreError> {
        let Some(entry) = self.agents.entry(agent_id).await.map_err(StoreError::KvGet)? else {
            return Ok(None);
        };
        if matches!(entry.operation, kv::Operation::Delete | kv::Operation::Purge) {
            return Ok(None);
        }
        let record = serde_json::from_slice(&entry.value).map_err(StoreError::Deserialize)?;
        Ok(Some(record))
    }

    async fn put_consent(&self, consent: ConsentRecord) -> Result<(), StoreError> {
        let key = consent_key(&consent.principal, &consent.agent_id, &consent.resource_iss);
        let bytes = serde_json::to_vec(&consent).map_err(StoreError::Serialize)?;
        self.consents.put(key, bytes.into()).await.map_err(StoreError::KvPut)?;
        Ok(())
    }

    async fn get_consent(
        &self,
        principal: &str,
        agent_id: &str,
        resource_iss: &str,
    ) -> Result<Option<ConsentRecord>, StoreError> {
        let key = consent_key(principal, agent_id, resource_iss);
        let Some(entry) = self.consents.entry(key).await.map_err(StoreError::KvGet)? else {
            return Ok(None);
        };
        if matches!(entry.operation, kv::Operation::Delete | kv::Operation::Purge) {
            return Ok(None);
        }
        let record = serde_json::from_slice(&entry.value).map_err(StoreError::Deserialize)?;
        Ok(Some(record))
    }
}

/// JetStream KV–backed [`ReplayStore`] for multi-replica AAuth deployments.
///
/// Replay protection is enforced by `kv::Store::create`, which atomically fails
/// when the key already exists (`CreateErrorKind::AlreadyExists`). Bucket-level
/// `max_age` bounds key lifetime; pass a TTL >= the longest expected
/// JWT / PoP nonce lifetime.
#[derive(Clone)]
pub struct JetStreamReplayStore {
    bucket: kv::Store,
}

impl JetStreamReplayStore {
    #[must_use]
    pub fn new(bucket: kv::Store) -> Self {
        Self { bucket }
    }

    /// Open (and optionally create) the replay bucket with the given max-age TTL.
    pub async fn open(
        js: &jetstream::Context,
        max_age: std::time::Duration,
        auto_create: bool,
    ) -> Result<Self, StoreError> {
        let bucket = open_or_create(js, replay_bucket_config(max_age), REPLAY_BUCKET, auto_create).await?;
        Ok(Self::new(bucket))
    }
}

#[must_use]
pub fn replay_bucket_config(max_age: std::time::Duration) -> kv::Config {
    kv::Config {
        bucket: REPLAY_BUCKET.to_owned(),
        history: 1,
        max_value_size: REPLAY_KEY_MAX_VALUE_SIZE,
        max_age,
        ..Default::default()
    }
}

#[async_trait]
impl ReplayStore for JetStreamReplayStore {
    async fn check_and_insert(&self, key: &str, _ttl_secs: u32) -> Result<bool, ReplayError> {
        match self.bucket.create(key, Bytes::from_static(b"1")).await {
            Ok(_) => Ok(true),
            Err(err) if err.kind() == kv::CreateErrorKind::AlreadyExists => Ok(false),
            Err(err) => Err(ReplayError::Backend(err.to_string())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_record_round_trips_through_kv_value() {
        let record = AgentRecord {
            agent_id: "acme/agent-1".to_string(),
            cnf_jwk: serde_json::json!({"kty": "OKP", "crv": "Ed25519", "x": "abc"}),
            principal: Some("alice".to_string()),
            iat: 1_700_000_000,
        };
        let bytes = serde_json::to_vec(&record).expect("serialize");
        let decoded: AgentRecord = serde_json::from_slice(&bytes).expect("deserialize");
        assert_eq!(record, decoded);
    }

    #[test]
    fn consent_record_round_trips_through_kv_value() {
        let record = ConsentRecord {
            principal: "alice".to_string(),
            agent_id: "acme/agent-1".to_string(),
            resource_iss: "https://res.example".to_string(),
            scope: "read".to_string(),
            exp: 1_700_000_000,
        };
        let bytes = serde_json::to_vec(&record).expect("serialize");
        let decoded: ConsentRecord = serde_json::from_slice(&bytes).expect("deserialize");
        assert_eq!(record, decoded);
    }

    #[test]
    fn consent_key_is_pipe_delimited() {
        assert_eq!(
            consent_key("alice", "acme/agent-1", "https://res.example"),
            "alice|acme/agent-1|https://res.example"
        );
    }

    #[test]
    fn bucket_configs_use_expected_names() {
        assert_eq!(agents_bucket_config().bucket, AGENTS_BUCKET);
        assert_eq!(consents_bucket_config().bucket, CONSENTS_BUCKET);
        assert_eq!(
            replay_bucket_config(std::time::Duration::from_secs(60)).bucket,
            REPLAY_BUCKET
        );
    }

    #[test]
    fn replay_bucket_config_carries_max_age() {
        let cfg = replay_bucket_config(std::time::Duration::from_secs(300));
        assert_eq!(cfg.max_age, std::time::Duration::from_secs(300));
        assert_eq!(cfg.max_value_size, REPLAY_KEY_MAX_VALUE_SIZE);
    }
}

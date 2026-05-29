//! In-memory config snapshots keyed by xDS node id, fed by NATS KV watches.

use std::collections::HashMap;
use std::sync::Arc;

use arc_swap::ArcSwap;
use async_nats::jetstream::kv::{self, Operation};
use futures::StreamExt;
use tokio::sync::watch;
use tokio::task::JoinHandle;

use crate::config::GatewayConfigSnapshot;
use crate::mapping::{self, MappedResources};

pub const DEFAULT_KV_BUCKET: &str = "mcp-gateway-config";
pub const DEFAULT_KV_KEY_PREFIX: &str = "xds/projection/";

#[derive(Debug, thiserror::Error)]
pub enum SnapshotError {
    #[error("snapshot not loaded for node `{node_id}`")]
    NotLoaded { node_id: String },
    #[error("failed to parse snapshot JSON: {0}")]
    InvalidJson(#[from] serde_json::Error),
    #[error("NATS KV error: {0}")]
    Kv(String),
}

#[derive(Clone, Debug)]
pub struct NodeSnapshot {
    pub node_id: String,
    pub config: GatewayConfigSnapshot,
    pub mapped: MappedResources,
}

impl NodeSnapshot {
    pub fn from_config(node_id: impl Into<String>, config: GatewayConfigSnapshot) -> Self {
        let mapped = mapping::map_snapshot(&config);
        Self {
            node_id: node_id.into(),
            config,
            mapped,
        }
    }
}

#[derive(Clone, Default)]
pub struct ConfigStore {
    inner: Arc<ArcSwap<HashMap<String, Arc<NodeSnapshot>>>>,
    revision: watch::Sender<String>,
}

impl ConfigStore {
    pub fn new(initial: HashMap<String, Arc<NodeSnapshot>>) -> (Self, watch::Receiver<String>) {
        let version = initial
            .values()
            .next()
            .map(|snap| snap.config.version.clone())
            .unwrap_or_else(|| "0".into());
        let (revision_tx, revision_rx) = watch::channel(version);
        (
            Self {
                inner: Arc::new(ArcSwap::from_pointee(initial)),
                revision: revision_tx,
            },
            revision_rx,
        )
    }

    pub fn upsert(&self, snapshot: NodeSnapshot) {
        let mut current = (**self.inner.load()).clone();
        let version = snapshot.config.version.clone();
        current.insert(snapshot.node_id.clone(), Arc::new(snapshot));
        self.inner.store(Arc::new(current));
        let _ = self.revision.send(version);
    }

    pub fn get(&self, node_id: &str) -> Option<Arc<NodeSnapshot>> {
        self.inner.load().get(node_id).cloned()
    }

    pub fn subscribe_revisions(&self) -> watch::Receiver<String> {
        self.revision.subscribe()
    }
}

pub struct ConfigWatcher {
    store: ConfigStore,
    _task: JoinHandle<()>,
}

impl ConfigWatcher {
    pub fn store(&self) -> ConfigStore {
        self.store.clone()
    }
}

pub async fn watch_kv(
    kv: kv::Store,
    key_prefix: impl Into<String>,
) -> Result<ConfigWatcher, SnapshotError> {
    let prefix = key_prefix.into();
    let (store, _rx) = ConfigStore::new(HashMap::new());
    seed_from_kv(&store, &kv, &prefix).await?;

    let store_clone = store.clone();
    let prefix_clone = prefix.clone();
    let task = tokio::spawn(async move {
        if let Err(error) = run_watch_loop(kv, store_clone, prefix_clone).await {
            tracing::error!(%error, "xDS KV watch loop exited");
        }
    });

    Ok(ConfigWatcher {
        store,
        _task: task,
    })
}

async fn seed_from_kv(
    store: &ConfigStore,
    kv: &kv::Store,
    prefix: &str,
) -> Result<(), SnapshotError> {
    let mut entries = kv
        .keys()
        .await
        .map_err(|error| SnapshotError::Kv(error.to_string()))?;
    while let Some(key) = entries
        .next()
        .await
        .transpose()
        .map_err(|error| SnapshotError::Kv(error.to_string()))?
    {
        if key.starts_with(prefix) {
            if let Some(entry) = kv
                .get(&key)
                .await
                .map_err(|error| SnapshotError::Kv(error.to_string()))?
            {
                apply_kv_value(store, &key, prefix, &entry)?;
            }
        }
    }
    Ok(())
}

async fn run_watch_loop(
    kv: kv::Store,
    store: ConfigStore,
    prefix: String,
) -> Result<(), SnapshotError> {
    let mut watcher = kv
        .watch(prefix.clone())
        .await
        .map_err(|error| SnapshotError::Kv(error.to_string()))?;
    while let Some(update) = watcher.next().await {
        let update = update.map_err(|error| SnapshotError::Kv(error.to_string()))?;
        if update.operation == Operation::Delete || update.operation == Operation::Purge {
            let mut current = (**store.inner.load()).clone();
            current.remove(&update.key);
            store.inner.store(Arc::new(current));
            continue;
        }
        apply_kv_value(&store, &update.key, &prefix, &update.value)?;
    }
    Ok(())
}

fn apply_kv_value(
    store: &ConfigStore,
    key: &str,
    prefix: &str,
    value: &[u8],
) -> Result<(), SnapshotError> {
    let node_id = key
        .strip_prefix(prefix)
        .unwrap_or(key)
        .trim_matches('/')
        .to_string();
    let config = GatewayConfigSnapshot::parse_json(value)?;
    store.upsert(NodeSnapshot::from_config(node_id, config));
    Ok(())
}

#[cfg(test)]
pub fn test_store_with_fixture(node_id: &str) -> ConfigStore {
    let snapshot = NodeSnapshot::from_config(
        node_id,
        crate::config::fixtures::sample_snapshot(),
    );
    let (store, _rx) = ConfigStore::new(HashMap::from([(node_id.to_string(), Arc::new(snapshot))]));
    store
}

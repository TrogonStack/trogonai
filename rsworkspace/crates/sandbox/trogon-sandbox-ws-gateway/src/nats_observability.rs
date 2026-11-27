use std::collections::{BTreeMap, HashMap};

use async_nats::Client;
use async_nats::jetstream;
use async_nats::jetstream::kv;
use futures::StreamExt;
use serde::Serialize;
use serde_json::Value;
use tokio_util::sync::CancellationToken;
use tracing::warn;
use trogon_sandbox_contract::{
    Digest, LeaseRecord, LiveSnapshotEntry, SnapshotKvRecord, SnapshotPublished, SnapshotTagRecord, TenantId,
    WorkerHeartbeat,
};
use trogon_sandbox_protocol::subjects::{
    SNAPSHOT_KV_BUCKET, SNAPSHOT_LIVE_KV_BUCKET, SNAPSHOT_PUBLISHED, SNAPSHOT_TAG_KV_BUCKET, snapshot_kv_key,
};
use trogon_sandbox_protocol::{legacy_tenant_scoped_kv_prefix, tenant_scoped_kv_key, tenant_scoped_kv_prefix};

use crate::constants::{STREAM_LEASES, STREAM_SNAPSHOTS, STREAM_WORKERS, WORKER_HEARTBEAT_ALL};
use crate::lease_event_body::LeaseEventBody;
use crate::lease_row::LeaseRow;
use crate::listings::{LeaseListing, SnapshotListing, TagListing};
use crate::snapshot_published_row::SnapshotPublishedRow;
use crate::snapshot_row::SnapshotRow;
use crate::stored_tag_entry::StoredTagEntry;
use crate::tag_row::TagRow;
use crate::worker_row::WorkerRow;
use crate::{EventSender, ObservabilityConfig, ObservabilityError};

/// Reads the broker's KV state and watches its change streams for the dashboard.
#[derive(Clone)]
pub struct NatsObservability {
    client: Client,
    jetstream: jetstream::Context,
    config: ObservabilityConfig,
}

impl NatsObservability {
    pub fn new(client: Client, config: ObservabilityConfig) -> Self {
        let jetstream = jetstream::new(client.clone());
        Self {
            client,
            jetstream,
            config,
        }
    }

    pub async fn list_leases(
        &self,
        tenant: &TenantId,
        tag_filter: &BTreeMap<String, String>,
    ) -> Result<Value, ObservabilityError> {
        let kv = self.open_kv(&self.config.active_lease_bucket).await?;
        let mut records = Vec::new();
        for key in self.kv_keys(&kv).await? {
            let entry = kv.entry(&key).await.map_err(ObservabilityError::read)?;
            let Some(entry) = entry else { continue };
            let record: LeaseRecord = serde_json::from_slice(&entry.value)?;
            if record.tenant == *tenant && record.tags.contains_all(tag_filter) {
                records.push(record);
            }
        }
        let mut leases: Vec<LeaseRow<'_>> = records.iter().map(LeaseRow::from).collect();
        leases.sort_by(|a, b| a.lease_id.as_str().cmp(b.lease_id.as_str()));
        Ok(serde_json::to_value(LeaseListing { leases })?)
    }

    pub async fn list_snapshots(&self, tenant: &TenantId) -> Result<Value, ObservabilityError> {
        let kv = self.open_kv(SNAPSHOT_KV_BUCKET).await?;
        let mut records: HashMap<Digest, (SnapshotKvRecord, bool)> = HashMap::new();
        for key in self.kv_keys(&kv).await? {
            let entry = live_entry(&kv, &key).await?;
            let Some(entry) = entry else { continue };
            let record: SnapshotKvRecord = serde_json::from_slice(&entry.value)?;
            if record.tenant != *tenant {
                continue;
            }
            let canonical = tenant_scoped_kv_key(&record.tenant, record.digest.as_str());
            if key != canonical && key != snapshot_kv_key(&record.digest) {
                continue;
            }
            let owned = key == canonical;
            if !owned && kv.entry(&canonical).await.map_err(ObservabilityError::read)?.is_some() {
                continue;
            }
            if records
                .get(&record.digest)
                .is_none_or(|(_, preferred)| owned || !preferred)
            {
                records.insert(record.digest.clone(), (record, owned));
            }
        }
        let mut lives = HashMap::new();
        for (digest, (record, _)) in &records {
            lives.insert(digest.clone(), self.fetch_live_snapshot(&record.tenant, digest).await);
        }
        let mut snapshots: Vec<SnapshotRow<'_>> = records
            .values()
            .map(|(record, _)| SnapshotRow::new(record, lives.get(&record.digest).and_then(Option::as_ref)))
            .collect();
        snapshots.sort_by(|a, b| a.digest.as_str().cmp(b.digest.as_str()));
        Ok(serde_json::to_value(SnapshotListing { snapshots })?)
    }

    pub async fn list_tags(&self, tenant: &TenantId) -> Result<Value, ObservabilityError> {
        let kv = self.open_kv(SNAPSHOT_TAG_KV_BUCKET).await?;
        let prefix = tenant_scoped_kv_prefix(tenant);
        let legacy_prefix = legacy_tenant_scoped_kv_prefix(tenant);
        let mut by_name: BTreeMap<String, SnapshotTagRecord> = BTreeMap::new();
        for key in self.kv_keys(&kv).await? {
            if !key.starts_with(&prefix) && !key.starts_with(&legacy_prefix) {
                continue;
            }
            let entry = live_entry(&kv, &key).await?;
            let Some(entry) = entry else { continue };
            let stored: StoredTagEntry = serde_json::from_slice(&entry.value)?;
            match stored.owner {
                // Stamped by the broker: trust it only for this tenant's key.
                Some(owner) => {
                    let name = owner.name.as_str().to_string();
                    if owner.tenant == *tenant && tenant_scoped_kv_key(&owner.tenant, &name) == key {
                        by_name.insert(name, stored.record);
                    }
                }
                // Written before the stamp: the legacy key is all that names it.
                None => {
                    if let Some(name) = key.strip_prefix(&legacy_prefix) {
                        by_name.entry(name.to_string()).or_insert(stored.record);
                    }
                }
            }
        }
        let tags = by_name
            .iter()
            .map(|(name, record)| TagRow::new(tenant, name.clone(), record))
            .collect();
        Ok(serde_json::to_value(TagListing { tags })?)
    }

    pub fn spawn_lease_watcher(&self, tenant: TenantId, tx: EventSender, cancel: CancellationToken) {
        tokio::spawn(watch_leases(self.clone(), tenant, tx, cancel));
    }

    pub fn spawn_snapshot_published_watcher(&self, tenant: TenantId, tx: EventSender, cancel: CancellationToken) {
        tokio::spawn(watch_published_snapshots(self.client.clone(), tenant, tx, cancel));
    }

    pub fn spawn_worker_heartbeat_watcher(&self, tx: EventSender, cancel: CancellationToken) {
        tokio::spawn(watch_worker_heartbeats(self.client.clone(), tx, cancel));
    }

    async fn open_kv(&self, bucket: &str) -> Result<kv::Store, ObservabilityError> {
        self.jetstream
            .get_key_value(bucket)
            .await
            .map_err(|source| ObservabilityError::Bucket {
                bucket: bucket.to_string(),
                source: Box::new(source),
            })
    }

    async fn kv_keys(&self, kv: &kv::Store) -> Result<Vec<String>, ObservabilityError> {
        let keys = kv.keys().await.map_err(ObservabilityError::read)?;
        Ok(keys.filter_map(|key| async move { key.ok() }).collect().await)
    }

    async fn fetch_live_snapshot(&self, tenant: &TenantId, digest: &Digest) -> Option<LiveSnapshotEntry> {
        let kv = self.open_kv(SNAPSHOT_LIVE_KV_BUCKET).await.ok()?;
        let row = live_entry(&kv, &tenant_scoped_kv_key(tenant, digest.as_str()))
            .await
            .ok()??;
        let value: Value = serde_json::from_slice(&row.value).ok()?;
        if value.get("deleted").and_then(Value::as_bool) == Some(true) {
            return None;
        }
        let entry: LiveSnapshotEntry = serde_json::from_value(value).ok()?;
        (entry.tenant == *tenant).then_some(entry)
    }
}

/// The current value of `key`, or `None` when it is absent or was deleted.
async fn live_entry(kv: &kv::Store, key: &str) -> Result<Option<kv::Entry>, ObservabilityError> {
    let entry = kv.entry(key).await.map_err(ObservabilityError::read)?;
    Ok(entry.filter(|entry| entry.operation == kv::Operation::Put))
}

async fn send_event(tx: &EventSender, stream: &str, body: &impl Serialize) -> bool {
    let event = serde_json::to_value(body);
    let Ok(event) = event else { return true };
    tx.send((stream.to_string(), event)).await.is_ok()
}

async fn watch_leases(observability: NatsObservability, tenant: TenantId, tx: EventSender, cancel: CancellationToken) {
    let Ok(kv) = observability.open_kv(&observability.config.active_lease_bucket).await else {
        return;
    };
    let watch = kv.watch_all().await;
    let Ok(mut watch) = watch else { return };
    loop {
        tokio::select! {
            () = cancel.cancelled() => break,
            entry = watch.next() => {
                let Some(entry) = entry else { break };
                let Ok(entry) = entry else { continue };
                if entry.operation != kv::Operation::Put {
                    continue;
                }
                match serde_json::from_slice::<LeaseRecord>(&entry.value) {
                    Ok(record) if record.tenant == tenant => {
                        let body = LeaseEventBody { action: "upsert", lease: LeaseRow::from(&record) };
                        if !send_event(&tx, STREAM_LEASES, &body).await {
                            break;
                        }
                    }
                    Ok(_) => {}
                    Err(error) => warn!(error = %error, "decode lease record from KV watch"),
                }
            }
        }
    }
}

async fn watch_published_snapshots(client: Client, tenant: TenantId, tx: EventSender, cancel: CancellationToken) {
    let Ok(mut subscription) = client.subscribe(SNAPSHOT_PUBLISHED).await else {
        return;
    };
    loop {
        tokio::select! {
            () = cancel.cancelled() => break,
            message = subscription.next() => {
                let Some(message) = message else { break };
                let Ok(event) = serde_json::from_slice::<SnapshotPublished>(&message.payload) else { continue };
                if event.tenant != tenant {
                    continue;
                }
                if !send_event(&tx, STREAM_SNAPSHOTS, &SnapshotPublishedRow::from(&event)).await {
                    break;
                }
            }
        }
    }
}

async fn watch_worker_heartbeats(client: Client, tx: EventSender, cancel: CancellationToken) {
    let Ok(mut subscription) = client.subscribe(WORKER_HEARTBEAT_ALL).await else {
        return;
    };
    loop {
        tokio::select! {
            () = cancel.cancelled() => break,
            message = subscription.next() => {
                let Some(message) = message else { break };
                let Ok(heartbeat) = serde_json::from_slice::<WorkerHeartbeat>(&message.payload) else { continue };
                if !send_event(&tx, STREAM_WORKERS, &WorkerRow::from(&heartbeat)).await {
                    break;
                }
            }
        }
    }
}

use std::collections::HashMap;

use async_nats::jetstream;
use async_nats::jetstream::kv;
use async_trait::async_trait;
use bytes::Bytes;
use futures::TryStreamExt;
use trogon_sandbox_contract::{LeaseId, LeaseRecord, TenantId};
use trogon_sandbox_protocol::subjects::LEASE_USAGE_KV_BUCKET;

use super::keys::{legacy_kv_key, nats_kv_key, nats_kv_tenant_prefix};
use super::{LeaseUsageStore, UsageEntry, UsageStoreError};
use crate::constants::USAGE_RETENTION;

pub struct NatsKvLeaseUsageStore {
    pub(super) kv: kv::Store,
    jetstream: jetstream::Context,
}

impl NatsKvLeaseUsageStore {
    pub async fn new(jetstream: jetstream::Context) -> Result<Self, UsageStoreError> {
        let kv = jetstream
            .create_or_update_key_value(kv::Config {
                bucket: LEASE_USAGE_KV_BUCKET.to_string(),
                history: 1,
                max_age: USAGE_RETENTION,
                ..Default::default()
            })
            .await?;
        Ok(Self { kv, jetstream })
    }
}

#[async_trait]
impl LeaseUsageStore for NatsKvLeaseUsageStore {
    async fn modify(
        &self,
        lease: &LeaseRecord,
        change: &(dyn for<'e> Fn(&'e mut UsageEntry) + Send + Sync),
    ) -> Result<(), UsageStoreError> {
        let key = nats_kv_key(&lease.tenant, &lease.lease_id);
        let legacy_key = legacy_kv_key(&lease.lease_id);
        for _ in 0..8 {
            let current = match self.kv.entry(&key).await? {
                Some(entry) if entry.operation == kv::Operation::Put => Some(entry),
                _ => None,
            };
            // An entry an older broker wrote under the bare lease id moves to
            // the tenant-scoped key on its first write here.
            let legacy = match &current {
                Some(_) => None,
                None => self.read_legacy(&legacy_key, &lease.tenant).await?,
            };
            let mut usage = match (&current, &legacy) {
                (Some(entry), _) => serde_json::from_slice::<UsageEntry>(&entry.value)?,
                (None, Some((usage, _))) => usage.clone(),
                (None, None) => UsageEntry::new(lease),
            };
            change(&mut usage);
            let payload = Bytes::from(serde_json::to_vec(&usage)?);
            // Revision-checked so a concurrent replica's change is never overwritten.
            let written = match current {
                Some(entry) => self
                    .kv
                    .update(&key, payload, entry.revision)
                    .await
                    .map(|_| ())
                    .map_err(|e| {
                        (
                            e.kind() == kv::UpdateErrorKind::WrongLastRevision,
                            UsageStoreError::from(e),
                        )
                    }),
                None => self
                    .kv
                    .create(&key, payload)
                    .await
                    .map(|_| ())
                    .map_err(|e| (e.kind() == kv::CreateErrorKind::AlreadyExists, UsageStoreError::from(e))),
            };
            match written {
                Ok(()) => {
                    if let Some((_, revision)) = legacy {
                        // Best effort: reads prefer the scoped entry, so a
                        // leftover legacy copy is never counted twice.
                        let _ = self.kv.delete_expect_revision(&legacy_key, Some(revision)).await;
                    }
                    return Ok(());
                }
                Err((true, _)) => continue,
                Err((false, error)) => return Err(error),
            }
        }
        Err(UsageStoreError::RetriesExhausted)
    }

    async fn get(&self, tenant: &TenantId, lease_id: &LeaseId) -> Result<Option<UsageEntry>, UsageStoreError> {
        match self.kv.entry(nats_kv_key(tenant, lease_id)).await? {
            Some(entry) if entry.operation == kv::Operation::Put => Ok(Some(serde_json::from_slice(&entry.value)?)),
            _ => Ok(self
                .read_legacy(&legacy_kv_key(lease_id), tenant)
                .await?
                .map(|(usage, _)| usage)),
        }
    }

    /// Reads only the tenant's own scoped keys (`tenant.<tenant>.>`) and the
    /// legacy bare-id ones (`*`), so other tenants' scoped entries are never
    /// listed or read. An entry present under both layouts counts once, as
    /// the scoped one.
    async fn tenant_entries(&self, tenant: &TenantId) -> Result<Vec<UsageEntry>, UsageStoreError> {
        let scoped_filter = format!("{}>", nats_kv_tenant_prefix(tenant));
        let mut scoped = HashMap::new();
        for usage in self.latest_entries(&scoped_filter, tenant).await? {
            scoped.insert(usage.lease_id.as_str().to_owned(), usage);
        }
        for usage in self.latest_entries("*", tenant).await? {
            scoped.entry(usage.lease_id.as_str().to_owned()).or_insert(usage);
        }
        Ok(scoped.into_values().collect())
    }
}

impl NatsKvLeaseUsageStore {
    /// The latest live entry of every key matching `filter` that belongs to
    /// `tenant`. Stops once the server's backlog at the time of the call is
    /// drained, so an empty match returns at once.
    async fn latest_entries(&self, filter: &str, tenant: &TenantId) -> Result<Vec<UsageEntry>, UsageStoreError> {
        let consumer = self
            .kv
            .stream
            .create_consumer(jetstream::consumer::push::OrderedConfig {
                deliver_subject: self.jetstream.client().new_inbox(),
                description: Some("lease usage tenant scan".to_string()),
                filter_subject: format!("{}{filter}", self.kv.prefix),
                replay_policy: jetstream::consumer::ReplayPolicy::Instant,
                deliver_policy: jetstream::consumer::DeliverPolicy::LastPerSubject,
                ..Default::default()
            })
            .await?;
        let mut found = Vec::new();
        if consumer.cached_info().num_pending == 0 {
            return Ok(found);
        }
        let mut messages = consumer.messages().await?;
        while let Some(message) = messages.try_next().await? {
            let pending = message
                .info()
                .map_err(|e| UsageStoreError::MessageInfo(e.to_string()))?
                .pending;
            let removed = message
                .headers
                .as_ref()
                .and_then(|h| h.get("KV-Operation"))
                .is_some_and(|op| matches!(op.as_str(), "DEL" | "PURGE"));
            if !removed
                && let Ok(usage) = serde_json::from_slice::<UsageEntry>(&message.payload)
                && &usage.tenant == tenant
            {
                found.push(usage);
            }
            if pending == 0 {
                break;
            }
        }
        Ok(found)
    }
    async fn read_usage(&self, key: &str) -> Result<Option<(UsageEntry, u64)>, UsageStoreError> {
        match self.kv.entry(key).await? {
            Some(entry) if entry.operation == kv::Operation::Put => {
                Ok(Some((serde_json::from_slice(&entry.value)?, entry.revision)))
            }
            _ => Ok(None),
        }
    }

    /// The legacy entry under `key`, if it belongs to `tenant`.
    async fn read_legacy(&self, key: &str, tenant: &TenantId) -> Result<Option<(UsageEntry, u64)>, UsageStoreError> {
        Ok(self.read_usage(key).await?.filter(|(usage, _)| &usage.tenant == tenant))
    }
}

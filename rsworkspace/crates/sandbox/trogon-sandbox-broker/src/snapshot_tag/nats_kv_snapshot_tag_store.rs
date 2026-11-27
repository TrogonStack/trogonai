use std::collections::HashMap;
use std::sync::Arc;
use std::time::SystemTime;

use async_nats::jetstream::{self, kv};
use async_trait::async_trait;
use bytes::Bytes;
use futures::StreamExt;
use tracing::warn;
use trogon_sandbox_contract::{
    Digest, ServiceAccountId, SnapshotTag, SnapshotTagAuditEvent, SnapshotTagName, SnapshotTagRecord, TenantId,
};
use trogon_sandbox_protocol::subjects::SNAPSHOT_TAG_KV_BUCKET;
use trogon_sandbox_protocol::{
    legacy_tenant_scoped_kv_prefix, parse_tenant_scoped_kv_key_any, tenant_scoped_kv_prefix,
};

use super::located_tag::LocatedTag;
use super::snapshot_tag_auditor::SnapshotTagAuditor;
use super::snapshot_tag_store::SnapshotTagStore;
use super::snapshot_tag_store_error::SnapshotTagStoreError;
use super::stored_tag_record::StoredTagRecord;
use super::tag_keys::{legacy_tag_kv_key, tag_kv_key};
use crate::constants::{SNAPSHOT_TAG_KV_HISTORY, SNAPSHOT_TAG_MAX_CAS_RETRIES, SNAPSHOT_TAG_RETENTION};

pub struct NatsKvSnapshotTagStore {
    pub kv: kv::Store,
    pub(crate) audit: Arc<dyn SnapshotTagAuditor>,
}

impl NatsKvSnapshotTagStore {
    pub async fn new(
        jetstream: jetstream::Context,
        audit: Arc<dyn SnapshotTagAuditor>,
    ) -> Result<Self, SnapshotTagStoreError> {
        let kv = jetstream
            .create_or_update_key_value(kv::Config {
                bucket: SNAPSHOT_TAG_KV_BUCKET.to_string(),
                history: SNAPSHOT_TAG_KV_HISTORY,
                max_age: SNAPSHOT_TAG_RETENTION,
                ..Default::default()
            })
            .await
            .map_err(SnapshotTagStoreError::backend)?;
        Ok(Self { kv, audit })
    }

    async fn read_entry(&self, key: &str) -> Result<Option<(StoredTagRecord, u64)>, SnapshotTagStoreError> {
        match self.kv.entry(key).await {
            Ok(None) => Ok(None),
            // A cleared tag leaves a delete or purge marker behind.
            Ok(Some(entry)) if entry.operation != kv::Operation::Put => Ok(None),
            Ok(Some(entry)) => {
                let stored: StoredTagRecord = serde_json::from_slice(&entry.value)
                    .map_err(|e| SnapshotTagStoreError::backend(format!("decode tag record: {e}")))?;
                Ok(Some((stored, entry.revision)))
            }
            Err(e) => Err(SnapshotTagStoreError::backend(e)),
        }
    }

    /// Finds `tag`'s own record. The current key is trusted when its record
    /// is stamped for `tag`, or unstamped and the key is also `tag`'s legacy
    /// key. Otherwise the legacy key is read, skipping a record stamped for
    /// another tag. An unstamped legacy record cannot name its owner, so it
    /// is accepted as before the key change (see EXPLANATION-kv-key-layout).
    async fn locate(&self, tag: &SnapshotTag) -> Result<Option<LocatedTag>, SnapshotTagStoreError> {
        let key = tag_kv_key(tag);
        let legacy = legacy_tag_kv_key(tag);
        if let Some((stored, revision)) = self.read_entry(&key).await? {
            let own = match &stored.owner {
                Some(owner) => owner == tag,
                None => legacy == key,
            };
            if own {
                return Ok(Some(LocatedTag {
                    key,
                    record: stored.record,
                    revision,
                }));
            }
        }
        if legacy == key {
            return Ok(None);
        }
        match self.read_entry(&legacy).await? {
            Some((stored, revision)) if stored.owner.as_ref().is_none_or(|owner| owner == tag) => {
                Ok(Some(LocatedTag {
                    key: legacy,
                    record: stored.record,
                    revision,
                }))
            }
            _ => Ok(None),
        }
    }

    fn encode(tag: &SnapshotTag, record: &SnapshotTagRecord) -> Result<Bytes, SnapshotTagStoreError> {
        let stored = StoredTagRecord {
            record: record.clone(),
            owner: Some(tag.clone()),
        };
        serde_json::to_vec(&stored)
            .map(Bytes::from)
            .map_err(|e| SnapshotTagStoreError::backend(format!("encode tag record: {e}")))
    }

    async fn emit_set_audit(&self, tag: &SnapshotTag, prev: Option<Digest>, record: &SnapshotTagRecord) {
        let event = SnapshotTagAuditEvent::Set {
            tag: tag.clone(),
            prev,
            next: record.digest.clone(),
            by: record.set_by.clone(),
            at: record.set_at,
        };
        if let Err(e) = self.audit.emit(event).await {
            warn!(error = %e, "failed to emit snapshot tag set audit event");
        }
    }

    async fn emit_cleared_audit(&self, tag: &SnapshotTag, prev: Digest, by: ServiceAccountId) {
        let event = SnapshotTagAuditEvent::Cleared {
            tag: tag.clone(),
            prev,
            by,
            at: SystemTime::now(),
        };
        if let Err(e) = self.audit.emit(event).await {
            warn!(error = %e, "failed to emit snapshot tag cleared audit event");
        }
    }
}

#[async_trait]
impl SnapshotTagStore for NatsKvSnapshotTagStore {
    async fn get(&self, tag: &SnapshotTag) -> Result<Option<SnapshotTagRecord>, SnapshotTagStoreError> {
        Ok(self.get_entry(tag).await?.map(|(record, _)| record))
    }

    async fn get_entry(&self, tag: &SnapshotTag) -> Result<Option<(SnapshotTagRecord, u64)>, SnapshotTagStoreError> {
        Ok(self
            .locate(tag)
            .await?
            .map(|located| (located.record, located.revision)))
    }

    async fn set(
        &self,
        tag: &SnapshotTag,
        record: SnapshotTagRecord,
        expected_revision: Option<u64>,
    ) -> Result<u64, SnapshotTagStoreError> {
        let key = tag_kv_key(tag);
        let payload = Self::encode(tag, &record)?;

        for _ in 0..SNAPSHOT_TAG_MAX_CAS_RETRIES {
            let Some(current) = self.locate(tag).await? else {
                if expected_revision.is_some_and(|revision| revision != 0) {
                    return Err(SnapshotTagStoreError::RevisionConflict);
                }
                match self.kv.create(&key, payload.clone()).await {
                    Ok(revision) => {
                        self.emit_set_audit(tag, None, &record).await;
                        return Ok(revision);
                    }
                    Err(e) if e.kind() == kv::CreateErrorKind::AlreadyExists => continue,
                    Err(e) => {
                        return Err(SnapshotTagStoreError::backend(format!("create tag record: {e}")));
                    }
                }
            };
            if let Some(expected) = expected_revision
                && current.revision != expected
            {
                return Err(SnapshotTagStoreError::RevisionConflict);
            }
            let prev = Some(current.record.digest.clone());
            if current.key == key {
                match self.kv.update(&key, payload.clone(), current.revision).await {
                    Ok(new_revision) => {
                        self.emit_set_audit(tag, prev, &record).await;
                        return Ok(new_revision);
                    }
                    Err(e) if e.kind() == kv::UpdateErrorKind::WrongLastRevision => continue,
                    Err(e) => {
                        return Err(SnapshotTagStoreError::backend(format!("update tag record: {e}")));
                    }
                }
            }
            // The record still lives under its legacy key: move it to the
            // current key, then retire the legacy entry at the revision read.
            let new_revision = match self.kv.create(&key, payload.clone()).await {
                Ok(revision) => revision,
                Err(e) if e.kind() == kv::CreateErrorKind::AlreadyExists => continue,
                Err(e) => {
                    return Err(SnapshotTagStoreError::backend(format!("create tag record: {e}")));
                }
            };
            if let Err(e) = self
                .kv
                .delete_expect_revision(&current.key, Some(current.revision))
                .await
            {
                warn!(error = %e, key = %current.key, "snapshot tag moved to its current key but the legacy entry was not removed");
            }
            self.emit_set_audit(tag, prev, &record).await;
            return Ok(new_revision);
        }

        Err(SnapshotTagStoreError::backend("tag store CAS failed after max retries"))
    }

    async fn clear(&self, tag: &SnapshotTag, expected_revision: u64) -> Result<(), SnapshotTagStoreError> {
        let Some(current) = self.locate(tag).await? else {
            return Err(SnapshotTagStoreError::NotFound);
        };
        if current.revision != expected_revision {
            return Err(SnapshotTagStoreError::RevisionConflict);
        }
        let prev = current.record.digest.clone();
        let by = current.record.set_by.clone();
        // The server rejects the delete if the tag moved past the revision
        // read above, so a set racing this clear is never lost.
        match self
            .kv
            .delete_expect_revision(&current.key, Some(current.revision))
            .await
        {
            Ok(()) => {}
            Err(e) if e.kind() == kv::DeleteErrorKind::WrongLastRevision => {
                return Err(SnapshotTagStoreError::RevisionConflict);
            }
            Err(e) => return Err(SnapshotTagStoreError::backend(format!("delete tag: {e}"))),
        }
        // A move interrupted between its create and its legacy delete leaves
        // the legacy entry behind; clearing it here keeps the tag from
        // reading back after the clear.
        if current.key == tag_kv_key(tag)
            && let Some(stale) = self.locate(tag).await?
            && let Err(e) = self.kv.delete_expect_revision(&stale.key, Some(stale.revision)).await
        {
            warn!(error = %e, key = %stale.key, "stale legacy snapshot tag entry was not removed");
        }
        self.emit_cleared_audit(tag, prev, by).await;
        Ok(())
    }

    async fn list(&self, tenant: &TenantId) -> Result<Vec<(SnapshotTag, SnapshotTagRecord)>, SnapshotTagStoreError> {
        let prefix = tenant_scoped_kv_prefix(tenant);
        let legacy_prefix = legacy_tenant_scoped_kv_prefix(tenant);
        let mut keys = self.kv.keys().await.map_err(SnapshotTagStoreError::backend)?;
        let mut stamped = HashMap::new();
        let mut unstamped = HashMap::new();
        while let Some(key) = keys.next().await {
            let key = key.map_err(SnapshotTagStoreError::backend)?;
            if !key.starts_with(&prefix) && !key.starts_with(&legacy_prefix) {
                continue;
            }
            let Some((stored, _)) = self.read_entry(&key).await? else {
                continue;
            };
            match stored.owner {
                Some(owner) => {
                    if owner.tenant == *tenant && tag_kv_key(&owner) == key {
                        stamped.insert(owner.name.clone(), (owner, stored.record));
                    }
                }
                None => {
                    // Pre-stamp record: the legacy key is all that names it.
                    if let Some(name) = key
                        .strip_prefix(&legacy_prefix)
                        .and_then(|name| SnapshotTagName::new(name).ok())
                    {
                        let tag = SnapshotTag {
                            tenant: tenant.clone(),
                            name,
                        };
                        unstamped.insert(tag.name.clone(), (tag, stored.record));
                    }
                }
            }
        }
        for (name, entry) in unstamped {
            stamped.entry(name).or_insert(entry);
        }
        let mut out: Vec<(SnapshotTag, SnapshotTagRecord)> = stamped.into_values().collect();
        out.sort_by(|a, b| a.0.name.cmp(&b.0.name));
        Ok(out)
    }

    async fn tenants(&self) -> Result<Vec<TenantId>, SnapshotTagStoreError> {
        let mut keys = self.kv.keys().await.map_err(SnapshotTagStoreError::backend)?;
        let mut set = std::collections::BTreeSet::new();
        while let Some(key) = keys.next().await {
            let key = key.map_err(SnapshotTagStoreError::backend)?;
            // A stamped record names its exact tenant, which also covers a
            // legacy key; an unstamped one falls back to the key itself.
            let Some((stored, _)) = self.read_entry(&key).await? else {
                continue;
            };
            let tenant = match stored.owner {
                Some(owner) => Some(owner.tenant),
                None => parse_tenant_scoped_kv_key_any(&key).map(|(t, _)| t),
            };
            set.extend(tenant);
        }
        Ok(set.into_iter().collect())
    }
}

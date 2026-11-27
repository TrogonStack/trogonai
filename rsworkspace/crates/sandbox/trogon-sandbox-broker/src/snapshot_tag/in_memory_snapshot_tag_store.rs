use std::collections::HashMap;
use std::sync::Arc;
use std::time::SystemTime;

use async_trait::async_trait;
use tokio::sync::Mutex;
use tracing::warn;
use trogon_sandbox_contract::{
    Digest, ServiceAccountId, SnapshotTag, SnapshotTagAuditEvent, SnapshotTagName, SnapshotTagRecord, TenantId,
};
use trogon_sandbox_protocol::{parse_tenant_scoped_kv_key, parse_tenant_scoped_kv_key_any};

use super::snapshot_tag_auditor::SnapshotTagAuditor;
use super::snapshot_tag_store::SnapshotTagStore;
use super::snapshot_tag_store_error::SnapshotTagStoreError;
use super::tag_keys::tag_kv_key;

struct TagSlot {
    record: SnapshotTagRecord,
    revision: u64,
}

pub struct InMemorySnapshotTagStore {
    tags: Mutex<HashMap<String, TagSlot>>,
    audit: Arc<dyn SnapshotTagAuditor>,
}

impl InMemorySnapshotTagStore {
    pub fn new(audit: Arc<dyn SnapshotTagAuditor>) -> Self {
        Self {
            tags: Mutex::new(HashMap::new()),
            audit,
        }
    }

    async fn emit_set(
        &self,
        tag: &SnapshotTag,
        prev: Option<Digest>,
        next: Digest,
        by: ServiceAccountId,
        at: SystemTime,
    ) {
        let event = SnapshotTagAuditEvent::Set {
            tag: tag.clone(),
            prev,
            next,
            by,
            at,
        };
        if let Err(e) = self.audit.emit(event).await {
            warn!(error = %e, "failed to emit snapshot tag set audit event");
        }
    }

    async fn emit_cleared(&self, tag: &SnapshotTag, prev: Digest, by: ServiceAccountId, at: SystemTime) {
        let event = SnapshotTagAuditEvent::Cleared {
            tag: tag.clone(),
            prev,
            by,
            at,
        };
        if let Err(e) = self.audit.emit(event).await {
            warn!(error = %e, "failed to emit snapshot tag cleared audit event");
        }
    }
}

#[async_trait]
impl SnapshotTagStore for InMemorySnapshotTagStore {
    async fn get(&self, tag: &SnapshotTag) -> Result<Option<SnapshotTagRecord>, SnapshotTagStoreError> {
        Ok(self.get_entry(tag).await?.map(|(record, _)| record))
    }

    async fn get_entry(&self, tag: &SnapshotTag) -> Result<Option<(SnapshotTagRecord, u64)>, SnapshotTagStoreError> {
        let key = tag_kv_key(tag);
        let tags = self.tags.lock().await;
        Ok(tags.get(&key).map(|slot| (slot.record.clone(), slot.revision)))
    }

    async fn set(
        &self,
        tag: &SnapshotTag,
        record: SnapshotTagRecord,
        expected_revision: Option<u64>,
    ) -> Result<u64, SnapshotTagStoreError> {
        let key = tag_kv_key(tag);
        let mut tags = self.tags.lock().await;

        match tags.get(&key) {
            None => {
                if expected_revision.is_some_and(|revision| revision != 0) {
                    return Err(SnapshotTagStoreError::RevisionConflict);
                }
                let revision = 1;
                let prev = None;
                tags.insert(
                    key,
                    TagSlot {
                        record: record.clone(),
                        revision,
                    },
                );
                drop(tags);
                self.emit_set(tag, prev, record.digest.clone(), record.set_by.clone(), record.set_at)
                    .await;
                Ok(revision)
            }
            Some(slot) => {
                if let Some(expected) = expected_revision
                    && slot.revision != expected
                {
                    return Err(SnapshotTagStoreError::RevisionConflict);
                }
                let prev = Some(slot.record.digest.clone());
                let revision = slot.revision.saturating_add(1);
                tags.insert(
                    key,
                    TagSlot {
                        record: record.clone(),
                        revision,
                    },
                );
                drop(tags);
                self.emit_set(tag, prev, record.digest.clone(), record.set_by.clone(), record.set_at)
                    .await;
                Ok(revision)
            }
        }
    }

    async fn clear(&self, tag: &SnapshotTag, expected_revision: u64) -> Result<(), SnapshotTagStoreError> {
        let key = tag_kv_key(tag);
        let mut tags = self.tags.lock().await;
        let slot = tags.get(&key).ok_or(SnapshotTagStoreError::NotFound)?;
        if slot.revision != expected_revision {
            return Err(SnapshotTagStoreError::RevisionConflict);
        }
        let prev = slot.record.digest.clone();
        let by = slot.record.set_by.clone();
        tags.remove(&key);
        drop(tags);
        self.emit_cleared(tag, prev, by, SystemTime::now()).await;
        Ok(())
    }

    async fn list(&self, tenant: &TenantId) -> Result<Vec<(SnapshotTag, SnapshotTagRecord)>, SnapshotTagStoreError> {
        let tags = self.tags.lock().await;
        let mut out: Vec<(SnapshotTag, SnapshotTagRecord)> = tags
            .iter()
            .filter_map(|(key, slot)| {
                parse_tenant_scoped_kv_key(key, tenant).and_then(|name| {
                    Some((
                        SnapshotTag {
                            tenant: tenant.clone(),
                            name: SnapshotTagName::new(name).ok()?,
                        },
                        slot.record.clone(),
                    ))
                })
            })
            .collect();
        out.sort_by(|a, b| a.0.name.cmp(&b.0.name));
        Ok(out)
    }

    async fn tenants(&self) -> Result<Vec<TenantId>, SnapshotTagStoreError> {
        let tags = self.tags.lock().await;
        let set: std::collections::BTreeSet<TenantId> = tags
            .keys()
            .filter_map(|key| parse_tenant_scoped_kv_key_any(key).map(|(t, _)| t))
            .collect();
        Ok(set.into_iter().collect())
    }
}

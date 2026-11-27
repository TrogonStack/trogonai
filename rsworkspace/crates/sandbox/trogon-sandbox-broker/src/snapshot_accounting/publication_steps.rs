use futures::StreamExt;
use trogon_sandbox_contract::{
    LiveSnapshotEntry, SnapshotAccountingDisposition, SnapshotAccountingRefusal as Refusal, SnapshotAccountingResponse,
    SnapshotPublished, TenantId,
};
use trogon_sandbox_protocol::subjects::SNAPSHOT_PUBLISHED;

use super::backend::Backend;
use super::confirmed_publication::ConfirmedPublication;
use super::live_artifact::LiveArtifact;
use super::refusal_result::Result;
use super::snapshot_accounting_store::SnapshotAccountingStore;
use crate::constants::{SNAPSHOT_ACCOUNTING_MAX_INTENT_BYTES, SNAPSHOT_ACCOUNTING_MAX_TENANTS};
use crate::quota::SnapshotQuota;

impl SnapshotAccountingStore {
    pub async fn apply_publication(
        &self,
        event: &SnapshotPublished,
        defaults: SnapshotQuota,
        legacy: Option<SnapshotQuota>,
    ) -> Result<ConfirmedPublication> {
        if let Some(observed) = legacy.as_ref()
            && (observed.count_used != 0 || observed.bytes_used != 0)
        {
            self.quarantine_legacy(&event.tenant, observed).await?;
            return Err(Refusal::LegacyUnconfirmed);
        }
        let generation = event.generation.as_ref().ok_or(Refusal::LegacyUnconfirmed)?;
        self.mutate(&event.tenant, |row| {
            if row.unconfirmed_legacy.is_some() {
                return Err(Refusal::LegacyUnconfirmed);
            }
            let capture = row.capture(generation)?;
            let binding = capture.binding.as_ref().ok_or(Refusal::Pending)?;
            if capture.abandoned
                || !capture.sealed
                || capture.intent.as_ref() != Some(event)
                || capture.reservation.lease_id != event.lease_id
                || binding.reference.digest != event.digest
                || binding.reference.artifact_type != event.artifact_type
                || binding.bytes != event.bytes
            {
                return Err(Refusal::MetadataMismatch);
            }
            let fence = row.fences.get(&event.digest).cloned().unwrap_or_default();
            let mut disposition = SnapshotAccountingDisposition::Live;
            if capture.reservation.sequence <= fence.closed_through {
                disposition = SnapshotAccountingDisposition::Deleted;
            } else if row
                .live
                .get(&event.digest)
                .is_some_and(|live| live.reservation_sequence > capture.reservation.sequence)
            {
                disposition = SnapshotAccountingDisposition::Superseded;
            } else if row
                .live
                .get(&event.digest)
                .is_some_and(|live| live.entry.generation.as_ref() == Some(generation))
            {
            } else {
                if fence.pending_delete.is_some() {
                    return Err(Refusal::Pending);
                }
                if row.quota.is_none() {
                    if legacy
                        .as_ref()
                        .is_some_and(|quota| quota.count_used != 0 || quota.bytes_used != 0)
                    {
                        return Err(Refusal::LegacyUnconfirmed);
                    }
                    row.quota = Some(legacy.clone().unwrap_or_else(|| defaults.clone()));
                }
                let existing = row.live.get(&event.digest);
                if existing.is_some_and(|live| {
                    live.entry.bytes != event.bytes || live.entry.artifact_type != event.artifact_type
                }) {
                    return Err(Refusal::MetadataMismatch);
                }
                if existing.is_none() {
                    let quota = row.quota.as_mut().ok_or(Refusal::Backend)?;
                    let count = quota.count_used.checked_add(1).ok_or(Refusal::QuotaExceeded)?;
                    let bytes = quota
                        .bytes_used
                        .checked_add(event.bytes)
                        .ok_or(Refusal::QuotaExceeded)?;
                    if count > quota.count_cap || bytes > quota.bytes_cap {
                        return Err(Refusal::QuotaExceeded);
                    }
                    quota.count_used = count;
                    quota.bytes_used = bytes;
                }
                let projection_revision = row.next()?;
                row.live.insert(
                    event.digest.clone(),
                    LiveArtifact {
                        entry: LiveSnapshotEntry {
                            tenant: event.tenant.clone(),
                            artifact_type: event.artifact_type,
                            bytes: event.bytes,
                            generation: Some(generation.clone()),
                            published_at: capture.reservation.created_at,
                        },
                        reservation_sequence: capture.reservation.sequence,
                        projection_revision,
                    },
                );
            }
            let live = row.live.get(&event.digest);
            Ok(ConfirmedPublication {
                response: SnapshotAccountingResponse::Committed {
                    tenant: event.tenant.clone(),
                    digest: event.digest.clone(),
                    lease_id: event.lease_id.clone(),
                    generation: generation.clone(),
                    disposition,
                },
                live: live.map(|record| record.entry.clone()),
                projection_revision: live.map_or(row.sequence, |record| record.projection_revision),
            })
        })
        .await
    }

    pub async fn stage_publication(&self, event: &SnapshotPublished) -> Result<()> {
        if serde_json::to_vec(event).map_err(|_| Refusal::Backend)?.len() > SNAPSHOT_ACCOUNTING_MAX_INTENT_BYTES {
            return Err(Refusal::Capacity);
        }
        let generation = event.generation.as_ref().ok_or(Refusal::LegacyUnconfirmed)?;
        self.mutate(&event.tenant, |row| {
            let capture = row
                .captures
                .values_mut()
                .find(|capture| capture.reservation.generation == *generation)
                .ok_or(Refusal::UnknownGeneration)?;
            let binding = capture.binding.as_ref().ok_or(Refusal::Pending)?;
            if capture.abandoned
                || capture.reservation.lease_id != event.lease_id
                || binding.reference.digest != event.digest
                || binding.reference.artifact_type != event.artifact_type
                || binding.bytes != event.bytes
            {
                return Err(Refusal::MetadataMismatch);
            }
            if capture.intent.as_ref().is_some_and(|intent| intent != event) {
                return Err(Refusal::MetadataMismatch);
            }
            capture.sealed = true;
            capture.intent = Some(event.clone());
            Ok(())
        })
        .await
    }

    pub async fn enqueue(&self, event: &SnapshotPublished) -> Result<()> {
        self.stage_publication(event).await?;
        let generation = event.generation.as_ref().ok_or(Refusal::LegacyUnconfirmed)?;
        let row = self.read(&event.tenant).await?.ok_or(Refusal::UnknownGeneration)?;
        if row.capture(generation)?.enqueued {
            return Ok(());
        }
        if let Backend::Nats { js, .. } = &self.backend {
            let payload = serde_json::to_vec(event).map_err(|_| Refusal::Backend)?;
            js.publish(SNAPSHOT_PUBLISHED, payload.into())
                .await
                .map_err(|_| Refusal::Backend)?
                .await
                .map_err(|_| Refusal::Backend)?;
        }
        self.mutate(&event.tenant, |row| {
            let capture = row
                .captures
                .values_mut()
                .find(|capture| capture.reservation.generation == *generation)
                .ok_or(Refusal::UnknownGeneration)?;
            if capture.intent.as_ref() != Some(event) {
                return Err(Refusal::MetadataMismatch);
            }
            capture.enqueued = true;
            Ok(())
        })
        .await
    }

    pub async fn recover_publications(&self) -> Result<usize> {
        let tenants: Vec<TenantId> = match &self.backend {
            Backend::Memory(rows) => rows.lock().await.keys().cloned().collect(),
            Backend::Nats { kv, .. } => {
                let mut keys = kv.keys().await.map_err(|_| Refusal::Backend)?;
                let mut tenants = Vec::new();
                while let Some(key) = keys.next().await {
                    let key = key.map_err(|_| Refusal::Backend)?;
                    let (tenant, _) = trogon_sandbox_protocol::parse_tenant_scoped_kv_key_any(&key)
                        .ok_or(Refusal::MetadataMismatch)?;
                    tenants.push(tenant);
                    if tenants.len() > SNAPSHOT_ACCOUNTING_MAX_TENANTS {
                        return Err(Refusal::Capacity);
                    }
                }
                tenants
            }
        };
        let mut recovered = 0;
        for tenant in tenants {
            let Some(row) = self.read(&tenant).await? else {
                continue;
            };
            for capture in row
                .captures
                .into_values()
                .filter(|capture| !capture.enqueued && !capture.abandoned)
            {
                if let Some(event) = capture.intent {
                    self.enqueue(&event).await?;
                    recovered += 1;
                }
            }
        }
        Ok(recovered)
    }
}

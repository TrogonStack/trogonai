use std::time::SystemTime;

use trogon_sandbox_contract::{
    LeaseId, OciArtifactRef, SnapshotAccountingRefusal as Refusal, SnapshotGeneration, SnapshotPublished,
    SnapshotReceipt, TenantId,
};

use super::capture_binding::CaptureBinding;
use super::capture_record::CaptureRecord;
use super::capture_reservation::CaptureReservation;
use super::refusal_result::Result;
use super::snapshot_accounting_store::SnapshotAccountingStore;
use crate::constants::{
    SNAPSHOT_ACCOUNTING_MAX_KEY_BYTES, SNAPSHOT_ACCOUNTING_MAX_RECEIPTS, SNAPSHOT_ACCOUNTING_MAX_REFERENCE_BYTES,
};

impl SnapshotAccountingStore {
    pub async fn reserve(
        &self,
        tenant: &TenantId,
        lease_id: &LeaseId,
        capture_key: &str,
    ) -> Result<CaptureReservation> {
        if capture_key.is_empty()
            || capture_key.len() > SNAPSHOT_ACCOUNTING_MAX_KEY_BYTES
            || lease_id.as_str().len() > SNAPSHOT_ACCOUNTING_MAX_KEY_BYTES
        {
            return Err(Refusal::Capacity);
        }
        let generation =
            SnapshotGeneration::new(crate::generated_ulid::generated_ulid()).map_err(|_| Refusal::Backend)?;
        let created_at = SystemTime::now();
        self.mutate(tenant, |row| {
            if let Some(current) = row.captures.get(capture_key) {
                if current.reservation.lease_id != *lease_id || current.abandoned {
                    return Err(Refusal::MetadataMismatch);
                }
                if current.binding.is_none() {
                    return Err(Refusal::Pending);
                }
                return Ok(current.reservation.clone());
            }
            if row.unconfirmed_legacy.is_some() {
                return Err(Refusal::LegacyUnconfirmed);
            }
            if row.fences.values().any(|fence| fence.pending_delete.is_some()) {
                return Err(Refusal::Pending);
            }
            if row.captures.len() >= SNAPSHOT_ACCOUNTING_MAX_RECEIPTS {
                return Err(Refusal::Capacity);
            }
            let reservation = CaptureReservation {
                tenant: tenant.clone(),
                lease_id: lease_id.clone(),
                capture_key: capture_key.to_string(),
                generation: generation.clone(),
                sequence: row.next()?,
                created_at,
            };
            row.captures.insert(
                capture_key.to_string(),
                CaptureRecord {
                    reservation: reservation.clone(),
                    binding: None,
                    abandoned: false,
                    sealed: false,
                    intent: None,
                    enqueued: false,
                },
            );
            Ok(reservation)
        })
        .await
    }

    pub async fn bind(
        &self,
        reservation: &CaptureReservation,
        reference: OciArtifactRef,
        receipt: &SnapshotReceipt,
    ) -> Result<()> {
        if reference.digest != receipt.digest
            || reference.artifact_type != receipt.artifact_type
            || reference.registry.len() > SNAPSHOT_ACCOUNTING_MAX_REFERENCE_BYTES
            || reference.repository.len() > SNAPSHOT_ACCOUNTING_MAX_REFERENCE_BYTES
        {
            return Err(Refusal::MetadataMismatch);
        }
        let binding = CaptureBinding {
            reference,
            bytes: receipt.bytes,
        };
        self.mutate(&reservation.tenant, |row| {
            let current = row
                .captures
                .get_mut(&reservation.capture_key)
                .ok_or(Refusal::UnknownGeneration)?;
            if current.reservation != *reservation || current.abandoned {
                return Err(Refusal::MetadataMismatch);
            }
            match &current.binding {
                Some(existing) if *existing != binding => Err(Refusal::MetadataMismatch),
                Some(_) => Ok(()),
                None => {
                    current.binding = Some(binding.clone());
                    Ok(())
                }
            }
        })
        .await
    }

    pub async fn abandon_unexported(&self, reservation: &CaptureReservation) -> Result<()> {
        self.mutate(&reservation.tenant, |row| {
            let current = row
                .captures
                .get_mut(&reservation.capture_key)
                .ok_or(Refusal::UnknownGeneration)?;
            if current.reservation != *reservation || current.binding.is_some() {
                return Err(Refusal::MetadataMismatch);
            }
            current.abandoned = true;
            Ok(())
        })
        .await
    }

    pub async fn pending_captures(&self, tenant: &TenantId) -> Result<Vec<CaptureReservation>> {
        Ok(self
            .read(tenant)
            .await?
            .map(|row| {
                row.captures
                    .into_values()
                    .filter(|capture| !capture.abandoned && capture.binding.is_none())
                    .map(|capture| capture.reservation)
                    .collect()
            })
            .unwrap_or_default())
    }

    pub async fn binding(&self, reservation: &CaptureReservation) -> Result<Option<CaptureBinding>> {
        let row = self
            .read(&reservation.tenant)
            .await?
            .ok_or(Refusal::UnknownGeneration)?;
        let capture = row.capture(&reservation.generation)?;
        if capture.reservation != *reservation || capture.abandoned {
            return Err(Refusal::MetadataMismatch);
        }
        Ok(capture.binding)
    }

    pub async fn seal(&self, reservation: &CaptureReservation) -> Result<()> {
        self.mutate(&reservation.tenant, |row| {
            let capture = row
                .captures
                .get_mut(&reservation.capture_key)
                .ok_or(Refusal::UnknownGeneration)?;
            if capture.reservation != *reservation || capture.abandoned || capture.binding.is_none() {
                return Err(Refusal::MetadataMismatch);
            }
            capture.sealed = true;
            if capture.intent.is_none() {
                let binding = capture.binding.as_ref().ok_or(Refusal::MetadataMismatch)?;
                capture.intent = Some(SnapshotPublished {
                    tenant: reservation.tenant.clone(),
                    lease_id: reservation.lease_id.clone(),
                    digest: binding.reference.digest.clone(),
                    artifact_type: binding.reference.artifact_type,
                    bytes: binding.bytes,
                    generation: Some(reservation.generation.clone()),
                    correlation_id: None,
                    tag: None,
                });
            }
            Ok(())
        })
        .await
    }

    pub async fn publication(&self, reservation: &CaptureReservation) -> Result<SnapshotPublished> {
        let row = self
            .read(&reservation.tenant)
            .await?
            .ok_or(Refusal::UnknownGeneration)?;
        let capture = row.capture(&reservation.generation)?;
        let binding = capture.binding.ok_or(Refusal::Pending)?;
        if capture.reservation != *reservation || capture.abandoned || !capture.sealed {
            return Err(Refusal::Pending);
        }
        Ok(SnapshotPublished {
            tenant: reservation.tenant.clone(),
            lease_id: reservation.lease_id.clone(),
            digest: binding.reference.digest,
            artifact_type: binding.reference.artifact_type,
            bytes: binding.bytes,
            generation: Some(reservation.generation.clone()),
            correlation_id: None,
            tag: None,
        })
    }
}

use std::collections::{BTreeMap, HashMap, HashSet};

use serde::{Deserialize, Serialize};
use trogon_sandbox_contract::{Digest, SnapshotAccountingRefusal as Refusal, SnapshotGeneration, TenantId};

use super::artifact_fence::ArtifactFence;
use super::capture_record::CaptureRecord;
use super::live_artifact::LiveArtifact;
use super::refusal_result::Result;
use super::snapshot_deletion_claim::SnapshotDeletionClaim;
use crate::constants::{SNAPSHOT_ACCOUNTING_MAX_RECEIPTS, SNAPSHOT_ACCOUNTING_MAX_RECORD_BYTES};
use crate::quota::SnapshotQuota;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct TenantArtifacts {
    pub(super) tenant: TenantId,
    pub(super) sequence: u64,
    pub(super) quota: Option<SnapshotQuota>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) unconfirmed_legacy: Option<SnapshotQuota>,
    pub(super) captures: BTreeMap<String, CaptureRecord>,
    pub(super) live: HashMap<Digest, LiveArtifact>,
    pub(super) fences: HashMap<Digest, ArtifactFence>,
    pub(super) deletions: BTreeMap<String, SnapshotDeletionClaim>,
}

impl TenantArtifacts {
    pub(super) fn new(tenant: TenantId) -> Self {
        Self {
            tenant,
            sequence: 0,
            quota: None,
            unconfirmed_legacy: None,
            captures: BTreeMap::new(),
            live: HashMap::new(),
            fences: HashMap::new(),
            deletions: BTreeMap::new(),
        }
    }
    pub(super) fn next(&mut self) -> Result<u64> {
        self.sequence = self.sequence.checked_add(1).ok_or(Refusal::Capacity)?;
        Ok(self.sequence)
    }
    pub(super) fn capture(&self, generation: &SnapshotGeneration) -> Result<CaptureRecord> {
        self.captures
            .values()
            .find(|record| record.reservation.generation == *generation)
            .cloned()
            .ok_or(Refusal::UnknownGeneration)
    }
    pub(super) fn validate(&self) -> Result<()> {
        let mut generations = HashSet::new();
        let mut sequences = HashSet::new();
        for (key, capture) in &self.captures {
            let reservation = &capture.reservation;
            if reservation.tenant != self.tenant
                || reservation.capture_key != *key
                || reservation.sequence == 0
                || reservation.sequence > self.sequence
                || !generations.insert(reservation.generation.clone())
                || !sequences.insert(reservation.sequence)
                || (capture.abandoned && capture.binding.is_some())
                || (capture.sealed && capture.binding.is_none())
            {
                return Err(Refusal::MetadataMismatch);
            }
            if let Some(event) = &capture.intent {
                let binding = capture.binding.as_ref().ok_or(Refusal::MetadataMismatch)?;
                if event.tenant != self.tenant
                    || event.lease_id != reservation.lease_id
                    || event.generation.as_ref() != Some(&reservation.generation)
                    || event.digest != binding.reference.digest
                    || event.bytes != binding.bytes
                    || event.artifact_type != binding.reference.artifact_type
                    || !capture.sealed
                {
                    return Err(Refusal::MetadataMismatch);
                }
            }
            if capture.enqueued && capture.intent.is_none() {
                return Err(Refusal::MetadataMismatch);
            }
        }
        for (digest, live) in &self.live {
            let generation = live.entry.generation.as_ref().ok_or(Refusal::MetadataMismatch)?;
            let capture = self.capture(generation)?;
            let binding = capture.binding.ok_or(Refusal::MetadataMismatch)?;
            if live.entry.tenant != self.tenant
                || live.entry.bytes != binding.bytes
                || live.entry.artifact_type != binding.reference.artifact_type
                || *digest != binding.reference.digest
                || live.reservation_sequence != capture.reservation.sequence
                || live.projection_revision > self.sequence
            {
                return Err(Refusal::MetadataMismatch);
            }
        }
        for (id, claim) in &self.deletions {
            if claim.tenant != self.tenant
                || *id != claim.claim_id
                || claim.sequence > self.sequence
                || claim.entry.tenant != self.tenant
                || claim.entry.generation.as_ref() != Some(&claim.generation)
                || claim.projection_revision > self.sequence
            {
                return Err(Refusal::MetadataMismatch);
            }
        }
        for fence in self.fences.values() {
            if fence.closed_through > self.sequence {
                return Err(Refusal::MetadataMismatch);
            }
            if let Some(id) = &fence.pending_delete
                && self.deletions.get(id).is_none_or(|claim| claim.completed)
            {
                return Err(Refusal::MetadataMismatch);
            }
        }
        if let Some(quota) = &self.quota {
            let bytes = self
                .live
                .values()
                .try_fold(0u64, |total, live| total.checked_add(live.entry.bytes))
                .ok_or(Refusal::MetadataMismatch)?;
            if quota.count_used != self.live.len() as u64 || quota.bytes_used != bytes {
                return Err(Refusal::MetadataMismatch);
            }
        } else if !self.live.is_empty() {
            return Err(Refusal::MetadataMismatch);
        }
        Ok(())
    }
    pub(super) fn encode(&self) -> Result<Vec<u8>> {
        self.validate()?;
        if self.captures.len() > SNAPSHOT_ACCOUNTING_MAX_RECEIPTS
            || self.deletions.len() > SNAPSHOT_ACCOUNTING_MAX_RECEIPTS
        {
            return Err(Refusal::Capacity);
        }
        if let Some(quota) = self.quota.as_ref() {
            let bytes = self
                .live
                .values()
                .try_fold(0u64, |total, live| total.checked_add(live.entry.bytes))
                .ok_or(Refusal::MetadataMismatch)?;
            if quota.count_used != self.live.len() as u64 || quota.bytes_used != bytes {
                return Err(Refusal::MetadataMismatch);
            }
        } else if !self.live.is_empty() {
            return Err(Refusal::MetadataMismatch);
        }
        let bytes = serde_json::to_vec(self).map_err(|_| Refusal::Backend)?;
        let limit = if self.unconfirmed_legacy.is_some() {
            SNAPSHOT_ACCOUNTING_MAX_RECORD_BYTES
        } else {
            SNAPSHOT_ACCOUNTING_MAX_RECORD_BYTES - 512
        };
        if bytes.len() > limit {
            return Err(Refusal::Capacity);
        }
        Ok(bytes)
    }
}

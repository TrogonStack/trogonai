use std::sync::Arc;
use std::time::SystemTime;

use bytes::Bytes;
use trogon_sandbox_contract::{
    LeaseError, LeaseId, OperationCancellation, OperationClaim, OperationId, OperationRecord, OperationRequest,
    OperationState, TenantId,
};

use super::begun::Begun;
use super::cancellation_intent::CancellationIntent;
use super::claim_token::ClaimToken;
use super::codec::encode;
use super::constants::{CAS_RETRIES, STALE_CLAIM_AFTER};
use super::keys::{active_key, cancellation_key, fingerprint, op_key, target_key};
use super::operation_store::OperationStore;
use super::operations_error::OperationsError;
use super::versioned::Versioned;
use crate::image_labels::operation_kind;

pub struct Operations {
    pub(super) store: Arc<dyn OperationStore>,
    pub(super) owner: String,
}

impl Operations {
    pub fn new(store: Arc<dyn OperationStore>, owner: impl Into<String>) -> Self {
        Self {
            store,
            owner: owner.into(),
        }
    }

    pub fn owner(&self) -> &str {
        &self.owner
    }

    pub async fn get(&self, tenant: &TenantId, id: &OperationId) -> Result<Option<Versioned>, OperationsError> {
        let Some(entry) = self.store.get(&op_key(tenant, id)).await? else {
            return Ok(None);
        };
        let record: OperationRecord = serde_json::from_slice(&entry.value)?;
        if record.tenant != *tenant || record.id != *id {
            return Err(OperationsError::RecordIdentityMismatch);
        }
        Ok(Some(Versioned {
            record,
            revision: entry.revision,
        }))
    }

    pub(super) async fn cancellation(
        &self,
        tenant: &TenantId,
        id: &OperationId,
    ) -> Result<Option<OperationCancellation>, OperationsError> {
        let Some(entry) = self.store.get(&cancellation_key(tenant, id)).await? else {
            return Ok(None);
        };
        let intent: CancellationIntent = serde_json::from_slice(&entry.value)?;
        if intent.tenant != *tenant || intent.operation_id != *id {
            return Err(OperationsError::CancellationIdentityMismatch);
        }
        Ok(Some(intent.cancellation))
    }

    pub async fn request_cancel(
        &self,
        tenant: &TenantId,
        id: &OperationId,
        now: SystemTime,
    ) -> Result<Option<Versioned>, OperationsError> {
        if let Some(found) = self.get(tenant, id).await?
            && found.record.state.is_terminal()
        {
            return Ok(Some(found));
        }
        let intent = CancellationIntent {
            tenant: tenant.clone(),
            operation_id: id.clone(),
            cancellation: OperationCancellation { requested_at: now },
        };
        self.store
            .create(&cancellation_key(tenant, id), encode(&intent)?)
            .await?;
        let cancellation = self
            .cancellation(tenant, id)
            .await?
            .ok_or(OperationsError::CancellationVanished)?;
        for _ in 0..CAS_RETRIES {
            let Some(found) = self.get(tenant, id).await? else {
                return Ok(None);
            };
            if found.record.state.is_terminal() || found.record.cancellation.is_some() {
                return Ok(Some(found));
            }
            if !matches!(found.record.request, Some(OperationRequest::Create { .. })) {
                return Err(OperationsError::CancellationUnsupported);
            }
            let mut record = found.record;
            record.cancellation = Some(cancellation.clone());
            record.next_attempt_at = None;
            if let Some(revision) = self
                .store
                .update(&op_key(tenant, id), encode(&record)?, found.revision)
                .await?
            {
                return Ok(Some(Versioned { record, revision }));
            }
        }
        Err(OperationsError::CancellationCasExhausted)
    }

    /// Records a new operation as queued and takes the lease for it.
    pub async fn begin(
        &self,
        tenant: &TenantId,
        id: &OperationId,
        lease: &LeaseId,
        request: OperationRequest,
        now: SystemTime,
    ) -> Result<Begun, OperationsError> {
        let print = fingerprint(tenant, lease, &request)?;
        if let Some(existing) = self.get(tenant, id).await? {
            return Ok(Self::existing(id, existing, &print));
        }
        if let Some(other) = self.take_target(tenant, id, lease).await? {
            return Ok(Begun::Conflict(LeaseError::OperationConflict {
                operation_id: id.clone(),
                reason: format!("lease {} already has operation {other} in flight", lease.as_str()),
            }));
        }
        let cancellation = if matches!(&request, OperationRequest::Create { .. }) {
            self.cancellation(tenant, id).await?
        } else {
            None
        };
        let record = OperationRecord {
            id: id.clone(),
            tenant: tenant.clone(),
            kind: operation_kind(&request),
            target: Some(lease.clone()),
            request_fingerprint: print.clone(),
            request: Some(request),
            state: OperationState::Queued,
            cancellation,
            allocation: None,
            compensation: None,
            phase: None,
            attempts: 0,
            claim: None,
            progress: None,
            result: None,
            error: None,
            requested_at: now,
            started_at: None,
            completed_at: None,
            deadline: None,
            next_attempt_at: None,
            gc_after: None,
        };
        match self.store.create(&op_key(tenant, id), encode(&record)?).await? {
            Some(revision) => {
                // The marker is what recovery scans; a lost race to create it is harmless.
                self.store
                    .create(&active_key(tenant, id), Bytes::from_static(b"{}"))
                    .await?;
                if matches!(record.request, Some(OperationRequest::Create { .. }))
                    && self.cancellation(tenant, id).await?.is_some()
                {
                    return Ok(Begun::Created(
                        self.request_cancel(tenant, id, now)
                            .await?
                            .ok_or(OperationsError::VanishedAfterCreation)?,
                    ));
                }
                Ok(Begun::Created(Versioned { record, revision }))
            }
            None => match self.get(tenant, id).await? {
                Some(existing) => Ok(Self::existing(id, existing, &print)),
                None => Err(OperationsError::VanishedDuringCreate),
            },
        }
    }

    fn existing(id: &OperationId, found: Versioned, print: &str) -> Begun {
        if found.record.request_fingerprint == print {
            Begun::Existing(found)
        } else {
            Begun::Conflict(LeaseError::OperationConflict {
                operation_id: id.clone(),
                reason: "the id was already used for a different request".to_string(),
            })
        }
    }

    /// Points the lease marker at this operation. `Some(other)` when a live
    /// operation already holds the lease; a marker left by a finished or
    /// missing operation is taken over.
    async fn take_target(
        &self,
        tenant: &TenantId,
        id: &OperationId,
        lease: &LeaseId,
    ) -> Result<Option<OperationId>, OperationsError> {
        let key = target_key(lease);
        let mine = encode(&(tenant, id))?;
        for _ in 0..CAS_RETRIES {
            let Some(entry) = self.store.get(&key).await? else {
                if self.store.create(&key, mine.clone()).await?.is_some() {
                    return Ok(None);
                }
                continue;
            };
            let (holder_tenant, holder): (TenantId, OperationId) = serde_json::from_slice(&entry.value)?;
            if holder_tenant == *tenant && holder == *id {
                return Ok(None);
            }
            let live = match self.get(&holder_tenant, &holder).await? {
                Some(found) => !found.record.state.is_terminal() && found.record.target.as_ref() == Some(lease),
                None => false,
            };
            if live {
                return Ok(Some(holder));
            }
            if self.store.update(&key, mine.clone(), entry.revision).await?.is_some() {
                return Ok(None);
            }
        }
        Err(OperationsError::LeaseMarkerCasExhausted)
    }

    /// Whether a record can be taken: queued, or running under a claim that no
    /// driver in this process holds. `is_driven_here` says whether one does.
    pub fn is_reclaimable(
        &self,
        record: &OperationRecord,
        now: SystemTime,
        foreign_claims_are_stale: bool,
        is_driven_here: bool,
    ) -> bool {
        match record.state {
            OperationState::Queued => record.next_attempt_at.is_none_or(|at| at <= now),
            OperationState::Running => match &record.claim {
                None => true,
                Some(claim) if claim.owner == self.owner => !is_driven_here,
                Some(claim) => {
                    foreign_claims_are_stale
                        || now
                            .duration_since(claim.renewed_at)
                            .is_ok_and(|age| age >= STALE_CLAIM_AFTER)
                }
            },
            _ => false,
        }
    }

    /// Takes the operation as running under the next generation. `None` when
    /// it is terminal or another writer got there first.
    pub async fn claim(
        &self,
        tenant: &TenantId,
        id: &OperationId,
        now: SystemTime,
    ) -> Result<Option<(OperationRecord, ClaimToken)>, OperationsError> {
        let Some(found) = self.get(tenant, id).await? else {
            return Ok(None);
        };
        let mut record = found.record;
        if record.cancellation.is_none() && matches!(record.request, Some(OperationRequest::Create { .. })) {
            record.cancellation = self.cancellation(tenant, id).await?;
        }
        if record.state.is_terminal() || !record.state.can_transition_to(OperationState::Running) {
            return Ok(None);
        }
        let generation = record
            .claim
            .as_ref()
            .map_or(0, |c| c.generation)
            .checked_add(1)
            .ok_or(OperationsError::ClaimGenerationExhausted)?;
        record.state = OperationState::Running;
        record.attempts = record.attempts.saturating_add(1);
        record.started_at.get_or_insert(now);
        record.next_attempt_at = None;
        record.claim = Some(OperationClaim {
            owner: self.owner.clone(),
            generation,
            renewed_at: now,
        });
        if self
            .store
            .update(&op_key(tenant, id), encode(&record)?, found.revision)
            .await?
            .is_none()
        {
            return Ok(None);
        }
        let token = ClaimToken {
            tenant: tenant.clone(),
            id: id.clone(),
            generation,
        };
        Ok(Some((record, token)))
    }
}

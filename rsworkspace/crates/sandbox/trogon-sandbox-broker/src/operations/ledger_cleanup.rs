use bytes::Bytes;
use trogon_sandbox_contract::{
    LeaseRecord, OperationAllocation, OperationId, OperationRequest, OperationState, TenantId,
};

use super::claim_token::ClaimToken;
use super::cleanup::{CleanupPhase, OwnedCleanup};
use super::codec::encode;
use super::constants::CAS_RETRIES;
use super::keys::{cleanup_active_key, cleanup_key, op_key};
use super::ledger::Operations;
use super::operations_error::OperationsError;

impl Operations {
    pub(crate) async fn cleanup(
        &self,
        tenant: &TenantId,
        id: &OperationId,
    ) -> Result<Option<OwnedCleanup>, OperationsError> {
        let Some(entry) = self.store.get(&cleanup_key(tenant, id)).await? else {
            return Ok(None);
        };
        let record: OwnedCleanup = serde_json::from_slice(&entry.value)?;
        if !(record.version == 1
            && record.tenant == *tenant
            && record.operation_id == *id
            && record.allocation.reservation.tenant == *tenant)
        {
            return Err(OperationsError::CleanupIdentityMismatch);
        }
        Ok(Some(record))
    }

    pub(crate) async fn begin_cleanup(
        &self,
        tenant: &TenantId,
        id: &OperationId,
        allocation: &OperationAllocation,
        source: &LeaseRecord,
    ) -> Result<OwnedCleanup, OperationsError> {
        let record = OwnedCleanup {
            version: 1,
            tenant: tenant.clone(),
            operation_id: id.clone(),
            allocation: allocation.clone(),
            phase: CleanupPhase::Requested,
            name: source.name.as_ref().map(ToString::to_string),
            drives: source
                .spec
                .drives()
                .iter()
                .map(|mount| mount.drive_id.clone())
                .collect(),
            monitoring: source.spec.retention().monitoring.is_retained(),
        };
        self.store
            .create(&cleanup_active_key(tenant, id), Bytes::from_static(b"{}"))
            .await?;
        self.store.create(&cleanup_key(tenant, id), encode(&record)?).await?;
        let found = self
            .cleanup(tenant, id)
            .await?
            .ok_or(OperationsError::CleanupIntentVanished)?;
        if found.allocation != *allocation {
            return Err(OperationsError::CleanupAllocationChanged);
        }
        if found.phase == CleanupPhase::Settled {
            self.store.delete(&cleanup_active_key(tenant, id), None).await?;
        }
        Ok(found)
    }

    pub(crate) async fn cleanup_requested(&self, tenant: &TenantId, id: &OperationId) -> Result<bool, OperationsError> {
        Ok(self.store.get(&cleanup_active_key(tenant, id)).await?.is_some())
    }

    pub(crate) async fn request_cleanup(&self, tenant: &TenantId, id: &OperationId) -> Result<(), OperationsError> {
        self.store
            .create(&cleanup_active_key(tenant, id), Bytes::from_static(b"{}"))
            .await?;
        Ok(())
    }

    pub(crate) async fn advance_cleanup(
        &self,
        tenant: &TenantId,
        id: &OperationId,
        allocation: &OperationAllocation,
        phase: CleanupPhase,
    ) -> Result<(), OperationsError> {
        for _ in 0..CAS_RETRIES {
            let key = cleanup_key(tenant, id);
            let entry = self
                .store
                .get(&key)
                .await?
                .ok_or(OperationsError::CleanupIntentMissing)?;
            let mut record = self
                .cleanup(tenant, id)
                .await?
                .ok_or(OperationsError::CleanupIntentVanished)?;
            if record.allocation != *allocation {
                return Err(OperationsError::CleanupAllocationChanged);
            }
            if record.phase >= phase {
                if phase == CleanupPhase::Settled {
                    self.store.delete(&cleanup_active_key(tenant, id), None).await?;
                }
                return Ok(());
            }
            record.phase = phase;
            if self
                .store
                .update(&key, encode(&record)?, entry.revision)
                .await?
                .is_some()
            {
                if phase == CleanupPhase::Settled {
                    self.store.delete(&cleanup_active_key(tenant, id), None).await?;
                }
                return Ok(());
            }
        }
        Err(OperationsError::CleanupCasExhausted)
    }

    pub(crate) async fn remember_allocation(
        &self,
        token: &ClaimToken,
        allocation: &OperationAllocation,
    ) -> Result<bool, OperationsError> {
        for _ in 0..CAS_RETRIES {
            let Some(found) = self.get(&token.tenant, &token.id).await? else {
                return Ok(false);
            };
            let mut record = found.record;
            if record.state != OperationState::Running
                || !record
                    .claim
                    .as_ref()
                    .is_some_and(|claim| claim.owner == self.owner && claim.generation == token.generation)
            {
                return Ok(false);
            }
            if let Some(previous) = &record.allocation {
                if previous != allocation {
                    return Err(OperationsError::AllocationChanged);
                }
                return Ok(true);
            }
            let expected_created_at = if allocation.target.is_some() {
                allocation.created_at
            } else {
                record.requested_at
            };
            if !(record.target.as_ref() == Some(&allocation.lease_id)
                && allocation.reservation.tenant == record.tenant
                && allocation.reservation.created_at == expected_created_at)
            {
                return Err(OperationsError::AllocationAuthorityMismatch);
            }
            let Some(OperationRequest::Create {
                attempt_id, dispatch, ..
            }) = &record.request
            else {
                return Err(OperationsError::AllocationWithoutCreate);
            };
            if allocation.reservation.attempt_id != *attempt_id {
                return Err(OperationsError::AllocationAttemptMismatch);
            }
            if let Some(dispatch) = dispatch {
                if dispatch.target != allocation.target {
                    return Err(OperationsError::AllocationAssignmentMismatch);
                }
            } else if allocation.target.is_some() {
                return Err(OperationsError::AllocationWithoutDispatch);
            }
            record.allocation = Some(allocation.clone());
            if self
                .store
                .update(&op_key(&token.tenant, &token.id), encode(&record)?, found.revision)
                .await?
                .is_some()
            {
                return Ok(true);
            }
        }
        Err(OperationsError::AllocationCasExhausted)
    }
}

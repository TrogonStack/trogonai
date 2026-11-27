use super::operation_cleanup_error::OperationCleanupError;
use super::*;
use crate::operations::{CleanupPhase, Operations};
use trogon_sandbox_contract::{CancelOperationResponse, OperationAllocation};

fn target(record: &LeaseRecord) -> Option<trogon_sandbox_contract::DispatchTarget> {
    record.dispatch_attempt.as_ref().and_then(|attempt| {
        Some(trogon_sandbox_contract::DispatchTarget {
            worker_id: record.worker_id.clone()?,
            attempt_id: attempt.id.clone(),
            epoch: attempt.epoch,
        })
    })
}

fn matches_allocation(record: &LeaseRecord, allocation: &OperationAllocation) -> bool {
    record.lease_id == allocation.lease_id
        && record.tenant == allocation.reservation.tenant
        && record.provider == allocation.provider
        && record.sandbox_id == allocation.sandbox_id.as_str()
        && record.created_at == allocation.created_at
        && reservation(record).as_ref() == Some(&allocation.reservation)
        && target(record) == allocation.target
}

fn reservation(record: &LeaseRecord) -> Option<trogon_sandbox_contract::AttemptQuotaReservation> {
    if record.dispatch_attempt.is_some() {
        BrokerCore::attempt_reservation(record).ok()
    } else {
        record.quota_reservation.clone()
    }
}

fn reservation_time_matches(record: &OperationRecord, allocation: &OperationAllocation) -> bool {
    allocation.reservation.created_at
        == if allocation.target.is_some() {
            allocation.created_at
        } else {
            record.requested_at
        }
}

impl BrokerCore {
    pub(super) async fn retain_create_allocation(
        &self,
        operations: &Operations,
        record: &OperationRecord,
        token: &crate::operations::ClaimToken,
        grant: &LeaseGranted,
    ) -> Result<(), OperationCleanupError> {
        if !grant.created.is_created() || record.target.as_ref() != Some(&grant.lease_id) {
            return Ok(());
        }
        let Some(OperationRequest::Create {
            attempt_id, dispatch, ..
        }) = &record.request
        else {
            return Ok(());
        };
        let Some(active) = self.active_leases.get(&grant.lease_id).await? else {
            return Ok(());
        };
        let Some(reservation) = reservation(&active) else {
            return Ok(());
        };
        if active.tenant != record.tenant
            || active.provider != grant.provider
            || active.sandbox_id != grant.sandbox_id
            || reservation.tenant != record.tenant
            || reservation.attempt_id != *attempt_id
            || reservation.created_at
                != if active.dispatch_attempt.is_some() {
                    active.created_at
                } else {
                    record.requested_at
                }
        {
            return Ok(());
        }
        if let Some(dispatch) = dispatch
            && dispatch.target != target(&active)
        {
            return Ok(());
        }
        let Ok(sandbox_id) = SandboxId::new(active.sandbox_id.clone()) else {
            return Ok(());
        };
        let allocation = OperationAllocation {
            lease_id: active.lease_id.clone(),
            provider: active.provider,
            sandbox_id,
            created_at: active.created_at,
            reservation: reservation.clone(),
            target: target(&active),
        };
        if !operations.remember_allocation(token, &allocation).await? {
            return Err(OperationCleanupError::ProvenanceClaimLost);
        }
        Ok(())
    }

    pub(super) async fn compensate_created_operation(
        &self,
        operations: &Operations,
        record: &OperationRecord,
    ) -> CancelOperationResponse {
        if record.state != OperationState::Succeeded {
            return CancelOperationResponse::AlreadyCompleted { state: record.state };
        }
        let Some(OperationResult::Granted(grant)) = &record.result else {
            return CancelOperationResponse::Unsupported;
        };
        if !grant.created.is_created() {
            return CancelOperationResponse::AlreadyCompleted { state: record.state };
        }
        if let Err(error) = operations.request_cleanup(&record.tenant, &record.id).await {
            return CancelOperationResponse::Denied(LeaseError::Backend {
                message: error.to_string(),
            });
        }
        let Some(allocation) = &record.allocation else {
            return CancelOperationResponse::Pending;
        };
        if record.target.as_ref() != Some(&grant.lease_id)
            || allocation.lease_id != grant.lease_id
            || allocation.provider != grant.provider
            || allocation.sandbox_id.as_str() != grant.sandbox_id
            || !reservation_time_matches(record, allocation)
            || allocation.reservation.tenant != record.tenant
        {
            return CancelOperationResponse::Pending;
        }
        match self.cleanup_operation_allocation(operations, record, allocation).await {
            Ok(()) => CancelOperationResponse::Canceled,
            Err(error) => {
                warn!(operation_id = %record.id, %error, "owned create compensation remains pending");
                CancelOperationResponse::Pending
            }
        }
    }

    pub(super) async fn cleanup_operation_allocation(
        &self,
        operations: &Operations,
        record: &OperationRecord,
        allocation: &OperationAllocation,
    ) -> Result<(), OperationCleanupError> {
        let _guard = self.attempt_lock(&allocation.lease_id).lock_owned().await;
        let cleanup = match operations.cleanup(&record.tenant, &record.id).await? {
            Some(cleanup) => {
                if cleanup.allocation != *allocation {
                    return Err(OperationCleanupError::IntentMismatch);
                }
                cleanup
            }
            None => {
                let source = self
                    .active_leases
                    .get(&allocation.lease_id)
                    .await?
                    .ok_or(OperationCleanupError::NoActiveIdentity)?;
                if !matches_allocation(&source, allocation) {
                    return Err(OperationCleanupError::IdentityChanged);
                }
                operations
                    .begin_cleanup(&record.tenant, &record.id, allocation, &source)
                    .await?
            }
        };
        if cleanup.phase == CleanupPhase::Settled {
            operations
                .advance_cleanup(&record.tenant, &record.id, allocation, CleanupPhase::Settled)
                .await?;
            return Ok(());
        }
        let current = self.active_leases.get(&allocation.lease_id).await?;
        if let Some(current) = &current
            && !matches_allocation(current, allocation)
        {
            return Err(OperationCleanupError::Replaced);
        }
        if cleanup.phase == CleanupPhase::Requested {
            if allocation.target.is_some() {
                let source = current.ok_or(OperationCleanupError::NoAttemptRecord)?;
                drop(_guard);
                self.cancel_targeted(&source, CancelIntent::Release, LeaseReleasedReason::Cancelled)
                    .await?;
                if let Some(name) = &cleanup.name {
                    self.named_leases
                        .clear_if_matches(&record.tenant, name, &allocation.lease_id)
                        .await?;
                }
                operations
                    .advance_cleanup(&record.tenant, &record.id, allocation, CleanupPhase::Settled)
                    .await?;
                return Ok(());
            }
            if allocation.provider != ProviderKind::E2b {
                return Err(OperationCleanupError::UnsupportedProvider);
            }
            if let Some(source) = current
                && source.status != LeaseStatus::Cancelling
            {
                let mut pending = source.clone();
                pending.status = LeaseStatus::Cancelling;
                if !self.active_leases.replace_if_unchanged(&source, &pending).await? {
                    return Err(OperationCleanupError::IntentRecordingLost);
                }
            }
            let request = trogon_sandbox_contract::CancelOwnedLeaseRequest {
                tenant: record.tenant.clone(),
                lease_id: allocation.lease_id.clone(),
                sandbox_id: allocation.sandbox_id.clone(),
            };
            let subject = subjects::provider_cancel_owned(allocation.provider, &allocation.lease_id);
            let reply = self
                .bus
                .request_with_timeout(
                    &subject,
                    Bytes::from(serde_json::to_vec(&request)?),
                    Duration::from_secs(30),
                )
                .await
                .map_err(OperationCleanupError::bus)?;
            let response: trogon_sandbox_contract::CancelOwnedLeaseResponse = serde_json::from_slice(&reply)?;
            let confirmed = matches!(
                response,
                trogon_sandbox_contract::CancelOwnedLeaseResponse::Cleaned { tenant, lease_id, sandbox_id }
                    if tenant == record.tenant && lease_id == allocation.lease_id && sandbox_id == allocation.sandbox_id
            );
            if !confirmed {
                return Err(OperationCleanupError::WorkerUnconfirmed);
            }
            operations
                .advance_cleanup(&record.tenant, &record.id, allocation, CleanupPhase::ProviderConfirmed)
                .await?;
        }
        if let Some(current) = self.active_leases.get(&allocation.lease_id).await? {
            if !matches_allocation(&current, allocation) {
                return Err(OperationCleanupError::BookkeepingChanged);
            }
            if current.status != LeaseStatus::Released {
                let mut released = current.clone();
                released.status = LeaseStatus::Released;
                if !self.active_leases.replace_if_unchanged(&current, &released).await? {
                    return Err(OperationCleanupError::SealingLost);
                }
            }
        }
        let refunded = self
            .quota
            .release_attempt(&allocation.reservation, &self.quota_limits())
            .await?;
        for drive in &cleanup.drives {
            self.drive_store.release_claim(drive, &allocation.lease_id).await?;
        }
        self.snapshot_pins
            .unpin(&allocation.lease_id)
            .await
            .map_err(OperationCleanupError::snapshot_pin)?;
        self.resolved_digests.clear(&allocation.lease_id).await?;
        self.command_launches.forget_lease(&allocation.lease_id).await?;
        if !cleanup.monitoring
            && let Some(logs) = &self.command_logs
        {
            logs.purge_lease(&allocation.lease_id).await?;
        }
        if let Some(name) = &cleanup.name {
            self.named_leases
                .clear_if_matches(&record.tenant, name, &allocation.lease_id)
                .await?;
        }
        self.revoke_credential_policy(&record.tenant, &allocation.lease_id)
            .await;
        if refunded {
            self.metrics.dec_active_leases(record.tenant.as_str());
        }
        operations
            .advance_cleanup(&record.tenant, &record.id, allocation, CleanupPhase::Settled)
            .await?;
        Ok(())
    }

    pub(super) async fn retain_canceled_allocation(
        &self,
        operations: &Operations,
        record: &OperationRecord,
        token: &crate::operations::ClaimToken,
        active: &LeaseRecord,
    ) -> Result<OperationAllocation, OperationCleanupError> {
        let Some(OperationRequest::Create {
            attempt_id, dispatch, ..
        }) = &record.request
        else {
            return Err(OperationCleanupError::NoCreateRequest);
        };
        let reservation = reservation(active).ok_or(OperationCleanupError::NoReservation)?;
        let identity_matches = record.target.as_ref() == Some(&active.lease_id)
            && active.tenant == record.tenant
            && !active.sandbox_id.is_empty()
            && reservation.tenant == record.tenant
            && reservation.attempt_id == *attempt_id
            && reservation.created_at
                == if active.dispatch_attempt.is_some() {
                    active.created_at
                } else {
                    record.requested_at
                };
        if !identity_matches {
            return Err(OperationCleanupError::ActiveIdentityDiffers);
        }
        if let Some(dispatch) = dispatch
            && dispatch.target != target(active)
        {
            return Err(OperationCleanupError::AssignmentDiffers);
        }
        let allocation = OperationAllocation {
            lease_id: active.lease_id.clone(),
            provider: active.provider,
            sandbox_id: SandboxId::new(active.sandbox_id.clone())?,
            created_at: active.created_at,
            reservation: reservation.clone(),
            target: target(active),
        };
        if !operations.remember_allocation(token, &allocation).await? {
            return Err(OperationCleanupError::AllocationClaimLost);
        }
        Ok(allocation)
    }
}

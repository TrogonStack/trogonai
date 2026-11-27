use std::time::SystemTime;

use trogon_sandbox_contract::{
    CreateQuotaStep, LeaseError, OperationId, OperationRecord, OperationRequest, OperationResult, OperationState,
    ProviderDispatchPayload, TenantId,
};

use super::claim_token::ClaimToken;
use super::codec::encode;
use super::constants::{CAS_RETRIES, OPERATION_RETENTION};
use super::keys::{active_key, op_key, parse_active_key, target_key};
use super::ledger::Operations;
use super::operations_error::OperationsError;
use super::settled::Settled;

impl Operations {
    /// Records the terminal outcome, only while `token` still holds the claim.
    pub async fn finish(
        &self,
        token: &ClaimToken,
        outcome: Result<OperationResult, LeaseError>,
        now: SystemTime,
    ) -> Result<Settled, OperationsError> {
        for _ in 0..CAS_RETRIES {
            let Some(found) = self.get(&token.tenant, &token.id).await? else {
                return Ok(Settled::Fenced);
            };
            let mut record = found.record;
            let holds = record
                .claim
                .as_ref()
                .is_some_and(|c| c.owner == self.owner && c.generation == token.generation);
            if !holds || record.state != OperationState::Running {
                return Ok(Settled::Fenced);
            }
            let canceled = record.cancellation.is_some()
                && matches!(&outcome, Err(LeaseError::OperationCanceled { operation_id }) if operation_id == &record.id);
            if record.cancellation.is_some() && !canceled {
                return Ok(Settled::Fenced);
            }
            match &outcome {
                Ok(result) => {
                    record.state = OperationState::Succeeded;
                    record.result = Some(result.clone());
                    record.error = None;
                }
                Err(error) => {
                    record.state = if canceled {
                        OperationState::Canceled
                    } else {
                        OperationState::Failed
                    };
                    record.error = Some(error.clone());
                    record.result = None;
                }
            }
            record.claim = None;
            // A finished create no longer needs its request, which can carry
            // environment values; the fingerprint and outcome are what replays use.
            if matches!(record.request, Some(OperationRequest::Create { .. })) {
                record.request = None;
            }
            record.completed_at = Some(now);
            record.gc_after = Some(now + OPERATION_RETENTION);
            if self
                .store
                .update(&op_key(&token.tenant, &token.id), encode(&record)?, found.revision)
                .await?
                .is_some()
            {
                self.release_markers(&record).await?;
                return Ok(Settled::Settled(Box::new(record)));
            }
        }
        Err(OperationsError::UpdateCasExhausted)
    }

    /// Puts a failed attempt back in the queue for a later one. The claim is
    /// kept so its generation keeps counting up, and the old token is fenced
    /// because the record is no longer running.
    pub async fn requeue(
        &self,
        token: &ClaimToken,
        reason: &LeaseError,
        retry_at: SystemTime,
    ) -> Result<Settled, OperationsError> {
        for _ in 0..CAS_RETRIES {
            let Some(found) = self.get(&token.tenant, &token.id).await? else {
                return Ok(Settled::Fenced);
            };
            let mut record = found.record;
            let holds = record
                .claim
                .as_ref()
                .is_some_and(|c| c.owner == self.owner && c.generation == token.generation);
            if !holds
                || record.state != OperationState::Running
                || !record.state.can_transition_to(OperationState::Queued)
            {
                return Ok(Settled::Fenced);
            }
            record.state = OperationState::Queued;
            record.next_attempt_at = Some(retry_at);
            record.progress = Some(format!("attempt {} failed: {reason}", record.attempts));
            if self
                .store
                .update(&op_key(&token.tenant, &token.id), encode(&record)?, found.revision)
                .await?
                .is_some()
            {
                return Ok(Settled::Settled(Box::new(record)));
            }
        }
        Err(OperationsError::UpdateCasExhausted)
    }

    /// Stores the dispatch intent before any worker send, only while `token`
    /// still holds the claim. `false` when the claim moved on.
    pub async fn save_dispatch(
        &self,
        token: &ClaimToken,
        payload: &ProviderDispatchPayload,
    ) -> Result<bool, OperationsError> {
        self.update_create(token, |dispatch, _| {
            *dispatch = Some(Box::new(payload.clone()));
        })
        .await
    }

    /// Records how far a create's quota accounting got, only while `token`
    /// still holds the claim. `None` clears it. `false` when the claim moved on.
    pub async fn save_quota_step(
        &self,
        token: &ClaimToken,
        step: Option<CreateQuotaStep>,
    ) -> Result<bool, OperationsError> {
        self.update_create(token, |_, quota| *quota = step.clone()).await
    }

    async fn update_create(
        &self,
        token: &ClaimToken,
        apply: impl Fn(&mut Option<Box<ProviderDispatchPayload>>, &mut Option<CreateQuotaStep>),
    ) -> Result<bool, OperationsError> {
        for _ in 0..CAS_RETRIES {
            let Some(found) = self.get(&token.tenant, &token.id).await? else {
                return Ok(false);
            };
            let mut record = found.record;
            let holds = record
                .claim
                .as_ref()
                .is_some_and(|c| c.owner == self.owner && c.generation == token.generation);
            let Some(OperationRequest::Create { dispatch, quota, .. }) = record.request.as_mut() else {
                return Ok(false);
            };
            if !holds || record.state != OperationState::Running {
                return Ok(false);
            }
            apply(dispatch, quota);
            if self
                .store
                .update(&op_key(&token.tenant, &token.id), encode(&record)?, found.revision)
                .await?
                .is_some()
            {
                return Ok(true);
            }
        }
        Err(OperationsError::UpdateCasExhausted)
    }

    /// Drops the active marker and the lease marker of a terminal operation.
    async fn release_markers(&self, record: &OperationRecord) -> Result<(), OperationsError> {
        self.store.delete(&active_key(&record.tenant, &record.id), None).await?;
        if let Some(lease) = &record.target {
            let key = target_key(lease);
            if let Some(entry) = self.store.get(&key).await? {
                let holder: Option<(TenantId, OperationId)> = serde_json::from_slice(&entry.value).ok();
                if holder.is_some_and(|(t, i)| t == record.tenant && i == record.id) {
                    self.store.delete(&key, Some(entry.revision)).await?;
                }
            }
        }
        Ok(())
    }

    /// Operations that have not reached a terminal state.
    pub async fn list_active(&self) -> Result<Vec<(TenantId, OperationId)>, OperationsError> {
        let mut active: Vec<_> = self
            .store
            .keys("active.")
            .await?
            .iter()
            .filter_map(|key| parse_active_key(key))
            .collect();
        active.extend(
            self.store
                .keys("cleanup_active.")
                .await?
                .iter()
                .filter_map(|key| parse_active_key(&key.replacen("cleanup_active.", "active.", 1))),
        );
        active.sort_by(|left, right| (left.0.as_str(), left.1.as_str()).cmp(&(right.0.as_str(), right.1.as_str())));
        active.dedup();
        Ok(active)
    }

    /// Removes the markers of an operation whose record is terminal or gone.
    pub async fn retire_markers(&self, tenant: &TenantId, id: &OperationId) -> Result<(), OperationsError> {
        match self.get(tenant, id).await? {
            Some(found) if found.record.state.is_terminal() => self.release_markers(&found.record).await,
            Some(_) => Ok(()),
            None => {
                self.store.delete(&active_key(tenant, id), None).await?;
                Ok(())
            }
        }
    }
}

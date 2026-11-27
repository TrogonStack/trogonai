//! Drives create, restore, pause, resume and delete as durable operations:
//! record the request, claim it, run the existing logic, store the outcome. A
//! crash anywhere in that sequence leaves a record the next run finds and
//! re-drives, which is safe because workers treat a repeated freeze or thaw as
//! success, a delete re-drives through the cancel path, and a create re-sends
//! the attempt and lease id it was enqueued with (see `create_operation`).

use super::constants::{MAX_UNSETTLED_RETRY_AFTER, POLL_EVERY, WAIT_SLACK};
use super::lock::lock;
use super::*;
use crate::operations::OperationsError;
use crate::operations::{Begun, ClaimToken, MAX_ATTEMPTS, Operations, RETRY_AFTER, Settled};
use trogon_sandbox_contract::CancelOperationResponse as Response;
use trogon_sandbox_contract::{
    RollbackLeaseRequest, RollbackLeaseResponse, SuspendLeaseRequest, SuspendLeaseResponse, WakeLeaseRequest,
    WakeLeaseResponse,
};

/// Marks an operation as driven by this process until dropped.
struct DrivingGuard<'a> {
    set: &'a std::sync::Mutex<std::collections::HashSet<String>>,
    key: String,
}

impl Drop for DrivingGuard<'_> {
    fn drop(&mut self) {
        if let Ok(mut set) = self.set.lock() {
            set.remove(&self.key);
        }
    }
}

pub(super) fn unexpected_result(result: &OperationResult) -> LeaseError {
    LeaseError::Backend {
        message: format!("operation stored an unexpected result: {result:?}"),
    }
}

/// What running an operation once came to.
pub(super) enum Outcome {
    /// Finished, with success or a definitive refusal.
    Done(Result<OperationResult, LeaseError>),
    /// A worker may have acted but no usable reply arrived. Try again, with
    /// the same ids, rather than fail.
    Retry(LeaseError),
    /// Resource effects remain unresolved, so ownership must outlive the
    /// ordinary retry limit.
    Unsettled(LeaseError),
}

fn replay(record: &OperationRecord) -> Result<(OperationId, OperationResult), LeaseError> {
    match (&record.state, &record.result, &record.error) {
        (OperationState::Succeeded, Some(result), _) => Ok((record.id.clone(), result.clone())),
        (_, _, Some(error)) => Err(error.clone()),
        _ => Err(LeaseError::Backend {
            message: format!("operation {} ended {:?} without a result", record.id, record.state),
        }),
    }
}

impl BrokerCore {
    pub async fn handle_cancel_operation(
        &self,
        req: trogon_sandbox_contract::CancelOperationRequest,
    ) -> trogon_sandbox_contract::CancelOperationResponse {
        let Some(operations) = self.operations.as_ref() else {
            return Response::Unsupported;
        };
        let found = match operations.get(&req.tenant, &req.operation_id).await {
            Ok(found) => found,
            Err(error) => return Response::Denied(backend(error)),
        };
        if let Some(found) = found {
            if found.record.state == OperationState::Canceled {
                return Response::Canceled;
            }
            if found.record.state.is_terminal() && req.compensate_created.is_compensate() {
                return self.compensate_created_operation(operations, &found.record).await;
            }
            if found.record.state.is_terminal() {
                return Response::AlreadyCompleted {
                    state: found.record.state,
                };
            }
            if !matches!(found.record.request, Some(OperationRequest::Create { .. })) {
                return Response::Unsupported;
            }
        }
        match operations
            .request_cancel(&req.tenant, &req.operation_id, SystemTime::now())
            .await
        {
            Err(error) => return Response::Denied(backend(error)),
            Ok(None) => return Response::Pending,
            Ok(Some(found)) if found.record.state.is_terminal() => {
                if req.compensate_created.is_compensate() {
                    return self.compensate_created_operation(operations, &found.record).await;
                }
                return if found.record.state == OperationState::Canceled {
                    Response::Canceled
                } else {
                    Response::AlreadyCompleted {
                        state: found.record.state,
                    }
                };
            }
            Ok(Some(_)) => {}
        }
        match self
            .drive_operation(operations, &req.tenant, &req.operation_id, false)
            .await
        {
            Ok(Some(record)) if record.state == OperationState::Canceled => Response::Canceled,
            Ok(Some(record)) if req.compensate_created.is_compensate() => {
                self.compensate_created_operation(operations, &record).await
            }
            Ok(Some(record)) => Response::AlreadyCompleted { state: record.state },
            Ok(None) => Response::Pending,
            Err(error) => Response::Denied(backend(error)),
        }
    }

    pub async fn handle_operation_capabilities(
        &self,
        _req: trogon_sandbox_contract::OperationCapabilitiesRequest,
    ) -> trogon_sandbox_contract::OperationCapabilities {
        trogon_sandbox_contract::OperationCapabilities {
            supports_create_cancellation: self.operations.is_some().into(),
        }
    }

    /// Records the request, drives it, and answers with its stored outcome.
    /// A repeated operation id replays the first outcome without running again.
    pub(super) async fn run_operation(
        &self,
        operations: &Operations,
        tenant: &TenantId,
        lease: &LeaseId,
        operation_id: Option<OperationId>,
        request: OperationRequest,
    ) -> Result<(OperationId, OperationResult), LeaseError> {
        let id = match operation_id {
            Some(id) => id,
            None => generated_operation_id().map_err(backend)?,
        };
        // An id that already exists is replayed or rejected by `begin`; only a
        // new one needs the ownership check, so a retry after the lease is gone still replays.
        let known = operations.get(tenant, &id).await.map_err(backend)?;
        if known.is_none() {
            self.owned_lease(tenant, lease).await?;
        }
        match operations
            .begin(tenant, &id, lease, request, SystemTime::now())
            .await
            .map_err(backend)?
        {
            Begun::Conflict(error) => Err(error),
            Begun::Created(_) | Begun::Existing(_) => self.await_operation(operations, tenant, &id).await,
        }
    }

    /// Drives an already recorded operation, or waits for whoever does, until
    /// it is terminal or the caller has waited long enough to be told to poll.
    pub(super) async fn await_operation(
        &self,
        operations: &Operations,
        tenant: &TenantId,
        id: &OperationId,
    ) -> Result<(OperationId, OperationResult), LeaseError> {
        let give_up = tokio::time::Instant::now() + self.config.provider_dispatch_timeout + WAIT_SLACK;
        loop {
            if let Some(record) = self
                .drive_operation(operations, tenant, id, false)
                .await
                .map_err(backend)?
            {
                return replay(&record);
            }
            if tokio::time::Instant::now() >= give_up {
                return Err(LeaseError::Backend {
                    message: format!("operation {id} is still running; poll get_operation for its outcome"),
                });
            }
            tokio::time::sleep(POLL_EVERY).await;
        }
    }

    /// Runs one operation if it is waiting for a driver. `Some` once it is
    /// terminal (whoever finished it); `None` when someone else holds it or the
    /// claim was lost, so the caller may poll again.
    pub(super) async fn drive_operation(
        &self,
        operations: &Operations,
        tenant: &TenantId,
        id: &OperationId,
        foreign_claims_are_stale: bool,
    ) -> Result<Option<OperationRecord>, OperationsError> {
        // Bound the future embedded in request and recovery callers as operation branches grow.
        Box::pin(self.drive_operation_inner(operations, tenant, id, foreign_claims_are_stale)).await
    }

    async fn drive_operation_inner(
        &self,
        operations: &Operations,
        tenant: &TenantId,
        id: &OperationId,
        foreign_claims_are_stale: bool,
    ) -> Result<Option<OperationRecord>, OperationsError> {
        let Some(found) = operations.get(tenant, id).await? else {
            return Ok(None);
        };
        if found.record.state.is_terminal() {
            if operations.cleanup_requested(tenant, id).await?
                && !matches!(
                    self.compensate_created_operation(operations, &found.record).await,
                    trogon_sandbox_contract::CancelOperationResponse::Canceled
                )
            {
                return Ok(None);
            }
            return Ok(Some(found.record));
        }
        let key = crate::operations::op_key(tenant, id);
        let _guard = {
            let mut driving = lock(&self.driving);
            let busy = driving.contains(&key);
            if busy || !operations.is_reclaimable(&found.record, SystemTime::now(), foreign_claims_are_stale, false) {
                return Ok(None);
            }
            driving.insert(key.clone());
            DrivingGuard {
                set: &self.driving,
                key,
            }
        };
        let Some((record, token)) = operations.claim(tenant, id, SystemTime::now()).await? else {
            return Ok(None);
        };
        let outcome = match self.execute_operation(&record, &token).await {
            Outcome::Unsettled(reason) => {
                let delay = if record.cancellation.is_some() {
                    RETRY_AFTER
                } else {
                    RETRY_AFTER
                        .saturating_mul(record.attempts)
                        .min(MAX_UNSETTLED_RETRY_AFTER)
                };
                operations.requeue(&token, &reason, SystemTime::now() + delay).await?;
                return Ok(None);
            }
            Outcome::Retry(reason) if record.attempts < MAX_ATTEMPTS => {
                let retry_at = SystemTime::now() + RETRY_AFTER;
                operations.requeue(&token, &reason, retry_at).await?;
                return Ok(None);
            }
            Outcome::Retry(reason) => Err(reason),
            Outcome::Done(result) => result,
        };
        if let Ok(OperationResult::Granted(grant)) = &outcome
            && let Err(error) = self.retain_create_allocation(operations, &record, &token, grant).await
        {
            let reason = backend(error);
            operations
                .requeue(&token, &reason, SystemTime::now() + RETRY_AFTER)
                .await?;
            return Ok(None);
        }
        // A re-drive that hit a transient backend failure (a worker still starting
        // after a restart) waits for another try; the first attempt answers its caller.
        if record.attempts > 1
            && record.attempts < MAX_ATTEMPTS
            && let Err(reason @ LeaseError::Backend { .. }) = &outcome
        {
            let retry_at = SystemTime::now() + RETRY_AFTER;
            operations.requeue(&token, reason, retry_at).await?;
            return Ok(None);
        }
        match operations.finish(&token, outcome, SystemTime::now()).await? {
            Settled::Settled(done) => {
                if done.state != OperationState::Succeeded
                    && matches!(record.request, Some(OperationRequest::Create { .. }))
                {
                    self.release_failed_create(&record).await;
                }
                Ok(Some(*done))
            }
            Settled::Fenced => {
                warn!(operation_id = %id, "operation claim was taken over; dropping this result");
                Ok(None)
            }
        }
    }

    /// Runs the existing verb logic for a recorded request. A re-drive (the
    /// operation has been attempted before) treats a repeat as success.
    async fn execute_operation(&self, record: &OperationRecord, token: &ClaimToken) -> Outcome {
        if matches!(record.request, Some(OperationRequest::Create { .. })) {
            if record.cancellation.is_some() {
                return self.cancel_create_operation(record, token).await;
            }
            return match self.operations.clone() {
                Some(operations) => self.execute_create(&operations, record, token).await,
                None => Outcome::Done(Err(LeaseError::Backend {
                    message: "durable operations are not enabled".to_string(),
                })),
            };
        }
        let result = self.execute_verb(record).await;
        // A dormancy dispatch that got no usable reply may have reached the
        // worker; the lease is left mid-transition and the retry finishes it.
        let dormancy_verb = matches!(
            record.request,
            Some(OperationRequest::Suspend { .. } | OperationRequest::Wake | OperationRequest::Rollback { .. })
        );
        match result {
            Err(error @ LeaseError::Backend { .. }) if dormancy_verb => Outcome::Retry(error),
            other => Outcome::Done(other),
        }
    }

    async fn execute_verb(&self, record: &OperationRecord) -> Result<OperationResult, LeaseError> {
        let (Some(lease_id), Some(request)) = (record.target.clone(), record.request.clone()) else {
            return Err(LeaseError::Backend {
                message: format!("operation {} has no request to run", record.id),
            });
        };
        let tenant = record.tenant.clone();
        let redrive = record.attempts > 1;
        match request {
            OperationRequest::Pause { max_paused_seconds } => match self
                .pause_lease_inline(PauseLeaseRequest {
                    tenant,
                    lease_id,
                    max_paused_seconds,
                    operation_id: None,
                })
                .await
            {
                PauseLeaseResponse::Paused { reap_at, .. } => Ok(OperationResult::Paused { reap_at }),
                PauseLeaseResponse::Unsupported => Ok(OperationResult::Unsupported),
                PauseLeaseResponse::Denied(error) => Err(error),
            },
            OperationRequest::Resume => match self
                .resume_lease_inline(ResumeLeaseRequest {
                    tenant,
                    lease_id,
                    operation_id: None,
                })
                .await
            {
                ResumeLeaseResponse::Resumed { deadline, .. } => Ok(OperationResult::Resumed { deadline }),
                ResumeLeaseResponse::Unsupported => Ok(OperationResult::Unsupported),
                ResumeLeaseResponse::Denied(error) => Err(error),
            },
            OperationRequest::Suspend {
                level,
                max_suspended_seconds,
            } => match self
                .suspend_lease_inline(
                    SuspendLeaseRequest {
                        tenant,
                        lease_id,
                        level,
                        max_suspended_seconds,
                        operation_id: None,
                    },
                    redrive,
                )
                .await
            {
                SuspendLeaseResponse::Suspended { level, reap_at, .. } => {
                    Ok(OperationResult::Suspended { level, reap_at })
                }
                SuspendLeaseResponse::Denied(error) => Err(error),
            },
            OperationRequest::Wake => match self
                .wake_lease_inline(
                    WakeLeaseRequest {
                        tenant,
                        lease_id,
                        operation_id: None,
                    },
                    redrive,
                )
                .await
            {
                WakeLeaseResponse::Woken { deadline, .. } => Ok(OperationResult::Woken { deadline }),
                WakeLeaseResponse::Denied(error) => Err(error),
            },
            OperationRequest::Delete => match self
                .delete_lease_inline(DeleteLeaseRequest {
                    tenant: tenant.clone(),
                    lease_id: lease_id.clone(),
                    operation_id: None,
                })
                .await
            {
                DeleteLeaseResponse::Deleted { .. } => Ok(OperationResult::Deleted),
                // The first attempt already released the lease; finish what it left undone.
                DeleteLeaseResponse::Denied(LeaseError::LeaseNotFound { .. }) if redrive => {
                    self.clear_deleted_lease_state(&lease_id, &tenant, None).await;
                    Ok(OperationResult::Deleted)
                }
                DeleteLeaseResponse::Denied(error) => Err(error),
            },
            OperationRequest::Rollback { snapshot } => match self
                .rollback_lease_inline(RollbackLeaseRequest {
                    tenant,
                    lease_id,
                    snapshot,
                    operation_id: None,
                })
                .await
            {
                RollbackLeaseResponse::RolledBack { .. } => Ok(OperationResult::RolledBack),
                RollbackLeaseResponse::Unsupported => Ok(OperationResult::Unsupported),
                RollbackLeaseResponse::Denied(error) => Err(error),
            },
            // Creates run through `execute_create`.
            OperationRequest::Create { .. } => Err(LeaseError::Backend {
                message: format!("operation {} is not a lifecycle verb", record.id),
            }),
        }
    }

    /// Re-drives every operation that has not finished. At startup, claims
    /// held by any other owner count as stale: the previous process is gone.
    /// Returns how many operations it took to a terminal state.
    pub async fn recover_operations(&self) -> usize {
        self.sweep_operations(true).await
    }

    /// Periodic pass over unfinished operations; takes over only claims that
    /// stopped renewing.
    pub async fn drive_pending_operations(&self) -> usize {
        self.sweep_operations(false).await
    }

    async fn sweep_operations(&self, foreign_claims_are_stale: bool) -> usize {
        let Some(operations) = self.operations.clone() else {
            return 0;
        };
        let active = match operations.list_active().await {
            Ok(active) => active,
            Err(error) => {
                warn!(%error, "listing unfinished operations failed");
                return 0;
            }
        };
        let mut finished = 0;
        for (tenant, id) in active {
            match self
                .drive_operation(&operations, &tenant, &id, foreign_claims_are_stale)
                .await
            {
                Ok(Some(_)) => finished += 1,
                Ok(None) => {}
                Err(error) => warn!(%error, operation_id = %id, "re-driving operation failed"),
            }
            // A terminal record whose marker survived a crash is cleaned here.
            if let Err(error) = operations.retire_markers(&tenant, &id).await {
                warn!(%error, operation_id = %id, "retiring operation markers failed");
            }
        }
        finished
    }

    /// Looks up one operation. Another tenant's id reads as not found.
    pub async fn handle_get_operation(&self, req: GetOperationRequest) -> GetOperationResponse {
        let Some(operations) = self.operations.as_ref() else {
            return GetOperationResponse::NotFound;
        };
        match operations.get(&req.tenant, &req.operation_id).await {
            Ok(Some(found)) => {
                let mut record = found.record;
                match operations.cleanup(&req.tenant, &req.operation_id).await {
                    Ok(Some(cleanup)) => {
                        record.compensation = Some(if cleanup.phase == crate::operations::CleanupPhase::Settled {
                            trogon_sandbox_contract::OperationCompensationState::Cleaned
                        } else {
                            trogon_sandbox_contract::OperationCompensationState::Pending
                        })
                    }
                    Ok(None) => match operations.cleanup_requested(&req.tenant, &req.operation_id).await {
                        Ok(true) => {
                            record.compensation = Some(trogon_sandbox_contract::OperationCompensationState::Pending)
                        }
                        Ok(false) => {}
                        Err(error) => return GetOperationResponse::Denied(backend(error)),
                    },
                    Err(error) => return GetOperationResponse::Denied(backend(error)),
                }
                GetOperationResponse::Found {
                    operation: Box::new(record),
                }
            }
            Ok(None) => GetOperationResponse::NotFound,
            Err(error) => GetOperationResponse::Denied(backend(error)),
        }
    }
}

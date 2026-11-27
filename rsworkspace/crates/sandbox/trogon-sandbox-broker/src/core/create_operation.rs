//! Create and restore as durable operations.
//!
//! A client opts in by sending `operation_id` on a lease request. The broker
//! then picks the lease id and the dispatch attempt id once, at enqueue time,
//! and stores them (with the typed request) in the operation record. Every
//! re-drive reuses them, so a repeat is recognised by whatever already saw the
//! first send:
//!
//! - Targeted providers: the attempt journal stays the source of truth and the
//!   operation points at it. The payload sent to the worker is stored on the
//!   operation before the send, so a re-drive resends exactly that attempt
//!   instead of waiting for the deadline reconciler.
//! - Legacy providers: the send intent is stored before dispatch. A missing reply
//!   remains unresolved until a matching Running receipt is reconciled, because
//!   a repeat can reach another replica with an independent worker-local guard.

use trogon_sandbox_contract::CreateQuotaStep;
use trogon_sandbox_contract::{DispatchAttemptId, LeaseAccounting, ProviderDispatchPayload};

use super::create_operation_error::CreateOperationError;
use super::*;
use crate::operations::{Begun, ClaimToken, Operations};

use super::dispatch_transaction::StoredDispatchOutcome;
use super::operation_driver::Outcome;

/// The operation ledger and the claim a create is running under.
pub(super) type OperationHandle = (Arc<Operations>, ClaimToken);

/// The ids a durable create keeps across re-drives.
pub(super) struct Preassigned {
    pub lease_id: LeaseId,
    pub attempt_id: DispatchAttemptId,
    pub requested_at: SystemTime,
}

/// Per-request state `lease_request_inner` reports back to its caller.
#[derive(Default)]
pub(super) struct CreateCtx {
    /// A name claimed before provisioning, released if the grant is denied.
    pub name_claim: Option<(TenantId, String, LeaseId)>,
    /// Set when a durable operation drives the create.
    pub preassigned: Option<Preassigned>,
    pub operation: Option<OperationHandle>,
    /// A worker may have launched although no usable reply came back.
    pub dispatch_unknown: bool,
    pub quota_reservation: Option<trogon_sandbox_contract::AttemptQuotaReservation>,
    pub quota_retry: bool,
    pub quota_release_requested: bool,
}

/// A lease id for an unnamed durable create: stable for the operation, so a
/// legacy worker guard sees every re-drive as the same lease. The tenant is
/// part of it because operation ids are only unique per tenant.
fn operation_lease_id(tenant: &TenantId, operation: &OperationId) -> LeaseId {
    super::generated_ids::lease_id_from_parts([tenant.as_str(), "create", operation.as_str()])
}

fn granted(record: &LeaseRecord) -> LeaseGranted {
    LeaseGranted {
        entity_id: record.entity_id.clone(),
        operation_id: None,
        lease_id: record.lease_id.clone(),
        provider: record.provider,
        sandbox_id: record.sandbox_id.clone(),
        evidence_subject: subjects::evidence_subject(&record.lease_id),
        deadline: record.deadline,
        created: LeaseCreation::Created,
        capabilities: Vec::new(),
    }
}

fn is_ours(journal: &LeaseRecord, lease_id: &LeaseId, attempt_id: &DispatchAttemptId) -> bool {
    journal.lease_id == *lease_id
        && journal
            .dispatch_attempt
            .as_ref()
            .is_some_and(|attempt| attempt.id == *attempt_id)
}

impl BrokerCore {
    pub(super) async fn cancel_create_operation(&self, record: &OperationRecord, token: &ClaimToken) -> Outcome {
        let canceled = LeaseError::OperationCanceled {
            operation_id: record.id.clone(),
        };
        let (
            Some(lease_id),
            Some(OperationRequest::Create {
                request,
                attempt_id,
                dispatch,
                quota,
            }),
        ) = (&record.target, &record.request)
        else {
            return Outcome::Unsettled(backend("canceled create has no retained allocation identity"));
        };
        let key = request.idempotency_key.clone();
        let journal = match self.idempotency.get_attempt(&record.tenant, &key).await {
            Ok(journal) => journal,
            Err(error) => return Outcome::Unsettled(backend(error.to_string())),
        };
        if let Some(journal) = journal {
            if !is_ours(&journal, lease_id, attempt_id) || journal.tenant != record.tenant {
                return Outcome::Unsettled(backend("canceled create attempt differs from its recorded authority"));
            }
            if let Err(error) = self
                .cancel_targeted(&journal, CancelIntent::Release, LeaseReleasedReason::Cancelled)
                .await
            {
                return Outcome::Unsettled(backend(format!("exact create cleanup remains pending: {error}")));
            }
            return Outcome::Done(Err(canceled));
        }
        let active = match self.active_leases.get(lease_id).await {
            Ok(active) => active,
            Err(error) => return Outcome::Unsettled(backend(error.to_string())),
        };
        if let Some(active) = active {
            let Some(operations) = self.operations.as_ref() else {
                return Outcome::Unsettled(backend("durable operation cleanup is unavailable"));
            };
            let allocation = match &record.allocation {
                Some(allocation) => allocation.clone(),
                None => match self
                    .retain_canceled_allocation(operations, record, token, &active)
                    .await
                {
                    Ok(allocation) => allocation,
                    Err(error) => return Outcome::Unsettled(backend(error.to_string())),
                },
            };
            return match self.cleanup_operation_allocation(operations, record, &allocation).await {
                Ok(()) => Outcome::Done(Err(canceled)),
                Err(error) => Outcome::Unsettled(backend(format!("owned create cleanup remains pending: {error}"))),
            };
        }
        if dispatch.is_some() {
            return Outcome::Unsettled(backend(
                "legacy create cleanup needs a positive exact owned-resource receipt",
            ));
        }
        if let Some(step) = quota {
            let reservation = match step {
                CreateQuotaStep::Reserved {
                    reservation: Some(reservation),
                    ..
                }
                | CreateQuotaStep::Releasing { reservation, .. } => reservation,
                _ => {
                    return Outcome::Unsettled(backend("canceled create quota has no exact releasable reservation"));
                }
            };
            if reservation.tenant != record.tenant
                || reservation.attempt_id != *attempt_id
                || reservation.created_at != record.requested_at
            {
                return Outcome::Unsettled(backend(
                    "canceled create reservation differs from its recorded authority",
                ));
            }
            if let Err(error) = self.quota.release_attempt(reservation, &self.quota_limits()).await {
                return Outcome::Unsettled(backend(format!(
                    "canceled create quota release remains pending: {error}"
                )));
            }
        }
        if let Some(name) = &request.name
            && let Err(error) = self
                .named_leases
                .clear_if_matches(&record.tenant, name.as_str(), lease_id)
                .await
        {
            return Outcome::Unsettled(backend(error.to_string()));
        }
        Outcome::Done(Err(canceled))
    }

    /// Records a create as an operation, drives it, and answers with its
    /// outcome. The same operation id again replays the first grant.
    pub(super) async fn create_operation(&self, mut req: LeaseRequest) -> LeaseResponse {
        let (Some(operations), Some(operation_id)) = (self.operations.clone(), req.operation_id.take()) else {
            return LeaseResponse::Denied(backend("durable operations are not enabled"));
        };
        // Cheap refusals first, so a malformed request is not recorded.
        if let Err(reason) = validate_spec(&req, self.lifetime.max_lease_lifetime_seconds) {
            return LeaseResponse::Denied(LeaseError::InvalidSpec {
                reason: reason.to_string(),
            });
        }
        let tenant = req.tenant.clone();
        let lease_id = match &req.name {
            Some(name) => derived_named_lease_id(&tenant, name.as_str(), req.idempotency_key.as_str()),
            None => operation_lease_id(&tenant, &operation_id),
        };
        let attempt_id = match generated_attempt_id() {
            Ok(attempt_id) => attempt_id,
            Err(error) => return LeaseResponse::Denied(backend(error.to_string())),
        };
        let request = OperationRequest::Create {
            request: Box::new(req),
            attempt_id,
            dispatch: None,
            quota: None,
        };
        let replay = match operations
            .begin(&tenant, &operation_id, &lease_id, request, SystemTime::now())
            .await
        {
            Err(error) => return LeaseResponse::Denied(backend(error.to_string())),
            Ok(Begun::Conflict(error)) => return LeaseResponse::Denied(error),
            Ok(Begun::Created(_)) => false,
            Ok(Begun::Existing(_)) => true,
        };
        match self.await_operation(&operations, &tenant, &operation_id).await {
            Ok((_, OperationResult::Granted(mut grant))) => {
                if replay {
                    grant.created = LeaseCreation::Existing;
                }
                grant.operation_id = Some(operation_id);
                LeaseResponse::Granted(grant)
            }
            Ok((_, other)) => LeaseResponse::Denied(unexpected_result(&other)),
            Err(error) => LeaseResponse::Denied(error),
        }
    }

    /// One drive of a create operation.
    pub(super) async fn execute_create(
        &self,
        operations: &Arc<Operations>,
        record: &OperationRecord,
        token: &ClaimToken,
    ) -> Outcome {
        let (
            Some(lease_id),
            Some(OperationRequest::Create {
                request,
                attempt_id,
                dispatch,
                quota,
            }),
        ) = (record.target.clone(), record.request.clone())
        else {
            return Outcome::Done(Err(backend(format!("operation {} has no request to run", record.id))));
        };
        let tenant = record.tenant.clone();
        let key = request.idempotency_key.clone();
        let legacy_send = dispatch.as_ref().is_some_and(|payload| payload.target.is_none());
        let has_progress = quota.is_some();
        let saved_reservation = match &quota {
            Some(CreateQuotaStep::Reserved { reservation, .. }) => reservation.clone(),
            Some(CreateQuotaStep::Releasing { reservation, error }) => {
                if reservation.tenant != tenant
                    || reservation.attempt_id != attempt_id
                    || reservation.created_at != record.requested_at
                {
                    return Outcome::Unsettled(backend("create refund receipt does not match the operation"));
                }
                return self.finish_create_quota_release(reservation, error).await;
            }
            _ => None,
        };

        if let Some(step) = quota
            && let Err(error) = self
                .reconcile_create_quota(operations, token, record, &request, &attempt_id, &lease_id, step)
                .await
        {
            return Outcome::Unsettled(backend(error.to_string()));
        }

        let mut confirmed_unsent = false;
        match self.idempotency.get_attempt(&tenant, &key).await {
            Err(error) => return Outcome::Unsettled(backend(error.to_string())),
            Ok(Some(journal)) if is_ours(&journal, &lease_id, &attempt_id) => {
                if let Some(outcome) = self.resume_attempt(journal, dispatch).await {
                    return outcome;
                }
                confirmed_unsent = true;
            }
            Ok(_) => {}
        }

        // A legacy create that crashed after it recorded the lease.
        match self.active_leases.get(&lease_id).await {
            Ok(Some(active)) if active.tenant == tenant && active.status == LeaseStatus::Running => {
                if (legacy_send || has_progress)
                    && (active.sandbox_id.is_empty()
                        || active.quota_reservation.as_ref().is_none_or(|reservation| {
                            reservation.tenant != tenant
                                || reservation.attempt_id != attempt_id
                                || reservation.created_at != record.requested_at
                        }))
                {
                    return Outcome::Unsettled(backend(
                        "create Running receipt does not match the original allocation",
                    ));
                }
                return Outcome::Done(Ok(OperationResult::Granted(granted(&active))));
            }
            Ok(_) => {}
            Err(error) => return Outcome::Unsettled(backend(error.to_string())),
        }

        if legacy_send || (has_progress && !confirmed_unsent) {
            return Outcome::Unsettled(backend(
                "create dispatch history is unconfirmed; reconcile the original allocation before retrying",
            ));
        }

        let mut ctx = CreateCtx {
            preassigned: Some(Preassigned {
                lease_id: lease_id.clone(),
                attempt_id: attempt_id.clone(),
                requested_at: record.requested_at,
            }),
            operation: Some((operations.clone(), token.clone())),
            quota_reservation: saved_reservation,
            ..CreateCtx::default()
        };
        let response = self.lease_request_inner(*request, &mut ctx).await;
        self.entity_finish_start(
            &tenant,
            ctx.name_claim.as_ref().map(|(_, name, _)| name.as_str()),
            ctx.name_claim.as_ref().map(|(_, _, lease)| lease),
            &response,
        )
        .await;
        match response {
            LeaseResponse::Granted(grant) => {
                if ctx.quota_reservation.is_some() && grant.lease_id != lease_id {
                    Outcome::Unsettled(backend(
                        "create reconnected another lease while its reservation remains unresolved",
                    ))
                } else {
                    Outcome::Done(Ok(OperationResult::Granted(grant)))
                }
            }
            LeaseResponse::Denied(error) => {
                if ctx.quota_release_requested
                    && let Some(reservation) = &ctx.quota_reservation
                {
                    let step = CreateQuotaStep::Releasing {
                        reservation: reservation.clone(),
                        error: error.clone(),
                    };
                    match operations.save_quota_step(token, Some(step)).await {
                        Ok(true) => {
                            return self.finish_create_quota_release(reservation, &error).await;
                        }
                        other => {
                            warn!(result = ?other, lease_id = %lease_id, "could not retain the create refusal before quota release");
                            return Outcome::Unsettled(error);
                        }
                    }
                }
                let in_flight = ctx.dispatch_unknown
                    || ctx.quota_retry
                    || ctx.quota_reservation.is_some()
                    || match self.idempotency.get_attempt(&tenant, &key).await {
                        Ok(Some(journal)) => {
                            is_ours(&journal, &lease_id, &attempt_id) && journal.status == LeaseStatus::Pending
                        }
                        Ok(None) => false,
                        Err(_) => true,
                    };
                if in_flight {
                    Outcome::Unsettled(error)
                } else {
                    Outcome::Done(Err(error))
                }
            }
        }
    }

    async fn finish_create_quota_release(
        &self,
        reservation: &trogon_sandbox_contract::AttemptQuotaReservation,
        denial: &LeaseError,
    ) -> Outcome {
        if let Err(error) = self.quota.release_attempt(reservation, &self.quota_limits()).await {
            warn!(%error, "create quota release remains pending");
            return Outcome::Unsettled(denial.clone());
        }
        Outcome::Done(Err(denial.clone()))
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "each argument is a distinct fact of one allocation; a bundle struct would only rename them"
    )]
    async fn reconcile_create_quota(
        &self,
        operations: &Arc<Operations>,
        token: &ClaimToken,
        operation: &OperationRecord,
        request: &LeaseRequest,
        attempt_id: &DispatchAttemptId,
        lease_id: &LeaseId,
        step: CreateQuotaStep,
    ) -> Result<(), CreateOperationError> {
        let CreateQuotaStep::Reserved {
            cpu_seconds,
            wall_seconds,
            reservation,
        } = step
        else {
            return Ok(());
        };
        let reservation = reservation.ok_or(CreateOperationError::NoReplayReceipt)?;
        let expected = trogon_sandbox_contract::AttemptQuotaReservation {
            tenant: operation.tenant.clone(),
            attempt_id: attempt_id.clone(),
            created_at: operation.requested_at,
            cpu_seconds,
            wall_seconds,
            accounting: if request.prewarm.is_prewarm() {
                LeaseAccounting::Prewarm
            } else {
                LeaseAccounting::Tenant
            },
        };
        if reservation != expected {
            return Err(CreateOperationError::ReceiptMismatchesOperation);
        }
        if let Some(active) = self.active_leases.get(lease_id).await? {
            if active.tenant != operation.tenant || active.quota_reservation.as_ref() != Some(&reservation) {
                return Err(CreateOperationError::ReceiptMismatchesLease);
            }
            let saved = operations
                .save_quota_step(token, Some(CreateQuotaStep::Counted))
                .await?;
            if !saved {
                return Err(CreateOperationError::ClaimLost);
            }
        }
        Ok(())
    }

    pub(super) async fn reserve_initial_quota(
        &self,
        ctx: &mut CreateCtx,
        tenant: &TenantId,
        cpu_seconds: u32,
        wall_seconds: u32,
        prewarm: LeasePrewarm,
    ) -> Result<(), LeaseError> {
        if let Some(preassigned) = &ctx.preassigned {
            let reservation =
                ctx.quota_reservation
                    .clone()
                    .unwrap_or_else(|| trogon_sandbox_contract::AttemptQuotaReservation {
                        tenant: tenant.clone(),
                        attempt_id: preassigned.attempt_id.clone(),
                        created_at: preassigned.requested_at,
                        cpu_seconds,
                        wall_seconds,
                        accounting: if prewarm.is_prewarm() {
                            LeaseAccounting::Prewarm
                        } else {
                            LeaseAccounting::Tenant
                        },
                    });
            ctx.quota_reservation = Some(reservation.clone());
            self.note_quota_reserved(ctx, cpu_seconds, wall_seconds).await?;
            if let Err(error) = self.quota.reserve_attempt(&reservation, &self.quota_limits()).await {
                self.refund_initial_quota(true, tenant, cpu_seconds, wall_seconds, ctx)
                    .await;
                return Err(error);
            }
            Ok(())
        } else {
            self.quota
                .check_and_reserve(
                    tenant,
                    cpu_seconds,
                    wall_seconds,
                    self.config.default_cpu_cap,
                    self.config.default_wall_cap,
                )
                .await
        }
    }

    pub(super) async fn note_quota_reserved(
        &self,
        ctx: &mut CreateCtx,
        cpu_seconds: u32,
        wall_seconds: u32,
    ) -> Result<(), LeaseError> {
        let Some((operations, token)) = &ctx.operation else {
            return Ok(());
        };
        let step = CreateQuotaStep::Reserved {
            cpu_seconds,
            wall_seconds,
            reservation: ctx.quota_reservation.clone(),
        };
        let failure = match operations.save_quota_step(token, Some(step)).await {
            Ok(true) => return Ok(()),
            Ok(false) => "operation claim was lost before the quota step was stored".to_string(),
            Err(error) => error.to_string(),
        };
        // A new claim may already be using the same reservation.
        ctx.quota_retry = true;
        Err(backend(failure))
    }

    /// The reservation already holds the active slot; this only records progress.
    pub(super) async fn note_quota_counted(&self, ctx: &CreateCtx) {
        if let Some((operations, token)) = &ctx.operation
            && let Err(error) = operations.save_quota_step(token, Some(CreateQuotaStep::Counted)).await
        {
            warn!(%error, "could not record the create quota as counted");
        }
    }

    /// Picks up an attempt the journal already holds for this operation.
    /// `None` means nothing was sent and the create starts over.
    async fn resume_attempt(
        &self,
        journal: LeaseRecord,
        dispatch: Option<Box<ProviderDispatchPayload>>,
    ) -> Option<Outcome> {
        match journal.status {
            LeaseStatus::Running | LeaseStatus::Paused => {
                if let Err(error) = self.active_leases.put(&journal).await {
                    return Some(Outcome::Unsettled(backend(error.to_string())));
                }
                Some(Outcome::Done(Ok(OperationResult::Granted(granted(&journal)))))
            }
            LeaseStatus::Pending => match dispatch.or_else(|| {
                // The operation's own copy is preferred; the journal keeps
                // one for an attempt whose operation never stored it.
                journal
                    .dispatch_attempt
                    .as_ref()
                    .and_then(|attempt| attempt.pending_dispatch.as_ref())
                    .and_then(|pending| serde_json::from_value(pending.payload.clone()).ok())
                    .map(Box::new)
            }) {
                Some(payload) => Some(self.resend_attempt(journal, &payload).await),
                None => match self.discard_unsent(&journal).await {
                    Ok(true) => None,
                    Ok(false) => Some(Outcome::Unsettled(backend(format!(
                        "attempt for lease {} changed while discarding it",
                        journal.lease_id.as_str()
                    )))),
                    Err(error) => Some(Outcome::Unsettled(backend(error.to_string()))),
                },
            },
            _ => Some(Outcome::Done(Err(backend(format!(
                "dispatch attempt for lease {} is {:?}; inspect or cancel that lease before retrying",
                journal.lease_id.as_str(),
                journal.status
            ))))),
        }
    }

    /// The crash came before the payload was stored, so no worker saw this
    /// attempt. Undo its reservations and free the journal key for a fresh try.
    async fn discard_unsent(&self, journal: &LeaseRecord) -> Result<bool, CreateOperationError> {
        let attempt = journal
            .dispatch_attempt
            .as_ref()
            .ok_or(CreateOperationError::MissingAttempt)?;
        let _admission = self.attempt_lock(&journal.lease_id).lock_owned().await;
        if !self
            .idempotency
            .forget_attempt(&journal.tenant, &attempt.idempotency_key, &journal.lease_id)
            .await?
        {
            return Ok(false);
        }
        for mount in journal.spec.drives() {
            self.drive_store
                .release_if_attached(&mount.drive_id, &journal.lease_id)
                .await?;
        }
        self.quota
            .release_attempt(&Self::attempt_reservation(journal)?, &self.quota_limits())
            .await?;
        self.snapshot_pins
            .unpin(&journal.lease_id)
            .await
            .map_err(CreateOperationError::snapshot_pin)?;
        self.resolved_digests.clear(&journal.lease_id).await?;
        self.active_leases.delete(&journal.lease_id).await?;
        Ok(true)
    }

    /// Sends the stored payload again through the same path the in-memory
    /// redrive uses. The worker keys on the attempt id, so a launch that
    /// already happened answers with its receipt.
    async fn resend_attempt(&self, journal: LeaseRecord, payload: &ProviderDispatchPayload) -> Outcome {
        let Some(target) = payload.target.clone() else {
            return Outcome::Done(Err(backend("stored dispatch has no target")));
        };
        if journal.worker_id.as_ref() != Some(&target.worker_id)
            || journal
                .dispatch_attempt
                .as_ref()
                .is_none_or(|attempt| attempt.id != target.attempt_id)
        {
            return Outcome::Done(Err(backend("stored dispatch targets another worker")));
        }
        if journal.deadline <= SystemTime::now() {
            // Too late to launch: the lease would be reaped on arrival.
            if let Err(error) = self
                .cancel_targeted(&journal, CancelIntent::Release, LeaseReleasedReason::Expired)
                .await
            {
                warn!(lease_id = %journal.lease_id, %error, "expired create attempt cleanup remains pending");
            }
            return Outcome::Done(Err(backend(format!(
                "lease {} expired before its dispatch could be re-sent",
                journal.lease_id.as_str()
            ))));
        }
        let bytes = match serde_json::to_vec(payload) {
            Ok(bytes) => Bytes::from(bytes),
            Err(error) => return Outcome::Done(Err(backend(error.to_string()))),
        };
        match self.send_stored_dispatch(&journal, bytes).await {
            StoredDispatchOutcome::Granted(grant) => Outcome::Done(Ok(OperationResult::Granted(grant))),
            StoredDispatchOutcome::Refused(denial) => Outcome::Done(Err(denial)),
            StoredDispatchOutcome::Unresolved(error) => Outcome::Unsettled(error),
        }
    }

    /// After a create ended failed: free its name and tear down any attempt
    /// still open, so nothing outlives an operation that will never finish.
    pub(super) async fn release_failed_create(&self, claimed: &OperationRecord) {
        let (
            Some(OperationRequest::Create {
                request, attempt_id, ..
            }),
            Some(lease_id),
        ) = (&claimed.request, &claimed.target)
        else {
            return;
        };
        let key = request.idempotency_key.clone();
        if let Ok(Some(journal)) = self.idempotency.get_attempt(&claimed.tenant, &key).await
            && is_ours(&journal, lease_id, attempt_id)
            && matches!(journal.status, LeaseStatus::Pending | LeaseStatus::Cancelling)
            && let Err(error) = self
                .cancel_targeted(&journal, CancelIntent::Release, LeaseReleasedReason::Failed)
                .await
        {
            warn!(lease_id = %lease_id, %error, "failed create attempt cleanup remains pending");
        }
        if let Some(name) = &request.name
            && let Err(error) = self
                .named_leases
                .clear_if_matches(&claimed.tenant, name.as_str(), lease_id)
                .await
        {
            warn!(%error, name = %name, lease_id = %lease_id, "failed to release the name of a failed create");
        }
    }
}

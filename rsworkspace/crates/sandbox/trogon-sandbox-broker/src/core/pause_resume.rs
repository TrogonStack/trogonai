use super::*;

impl BrokerCore {
    /// Sends a freeze (`pause`) or thaw (`resume`) to the worker holding the
    /// lease and folds the reply into a typed outcome.
    async fn dispatch_freeze(
        &self,
        provider: ProviderKind,
        lease_id: &LeaseId,
        action: FreezeAction,
    ) -> Result<(), SandboxFreezeResponse> {
        let thaw = action.resumes();
        let body = serde_json::to_vec(&SandboxFreezeRequest {
            lease_id: lease_id.clone(),
        })
        .map(Bytes::from)
        .map_err(|e| SandboxFreezeResponse::Denied(LeaseError::Backend { message: e.to_string() }))?;
        let subject = if thaw {
            subjects::provider_resume(provider, lease_id)
        } else {
            subjects::provider_pause(provider, lease_id)
        };
        let reply = self.bus.request(&subject, body).await.map_err(|e| {
            warn!(error = %e, lease_id = %lease_id, thaw, "freeze dispatch failed");
            SandboxFreezeResponse::Denied(LeaseError::Backend { message: e.to_string() })
        })?;
        match serde_json::from_slice::<SandboxFreezeResponse>(&reply) {
            Ok(SandboxFreezeResponse::Done) => Ok(()),
            Ok(other) => Err(other),
            Err(e) => Err(SandboxFreezeResponse::Denied(LeaseError::Backend {
                message: format!("freeze reply decode failed: {e}"),
            })),
        }
    }

    /// Freezes a running lease in place, keeping its id and state. The run
    /// deadline is parked in the record and replaced by a paused budget so the
    /// sweeper does not reap the lease while it is paused. Idempotent.
    /// The pause itself, without recording intent. The entity reconciler runs
    /// this for a `Paused` entity, whose intent is already on the record.
    pub(super) async fn drive_pause_lease(&self, req: PauseLeaseRequest) -> PauseLeaseResponse {
        let Some(operations) = self.operations.clone() else {
            return self.pause_lease_inline(req).await;
        };
        match self
            .run_operation(
                &operations,
                &req.tenant,
                &req.lease_id,
                req.operation_id,
                OperationRequest::Pause {
                    max_paused_seconds: req.max_paused_seconds,
                },
            )
            .await
        {
            Ok((operation_id, OperationResult::Paused { reap_at })) => PauseLeaseResponse::Paused {
                reap_at,
                operation_id: Some(operation_id),
            },
            Ok((_, OperationResult::Unsupported)) => PauseLeaseResponse::Unsupported,
            Ok((_, other)) => PauseLeaseResponse::Denied(unexpected_result(&other)),
            Err(error) => PauseLeaseResponse::Denied(error),
        }
    }

    /// Pauses without recording an operation. The operation driver and the
    /// no-store fallback run this.
    pub(crate) async fn pause_lease_inline(&self, req: PauseLeaseRequest) -> PauseLeaseResponse {
        self.pause_lease(req, false).await
    }

    /// Whether the provider's capability profile allows pausing. An unknown
    /// provider is treated as able, matching what [`Self::handle_pause_lease`]
    /// attempts.
    pub(crate) async fn provider_can_pause(&self, provider: ProviderKind) -> bool {
        let requirements = CapabilityRequirements {
            needs_pause_resume: CapabilityNeed::Required,
            ..Default::default()
        };
        self.worker_registry
            .profile_for_provider(provider)
            .await
            .is_none_or(|profile| profile.satisfies(&requirements))
    }

    /// Backs off the next idle pause of a lease whose idle pause tried at `now`
    /// did not take effect. Best effort: without the write the next sweep
    /// simply tries again.
    pub(crate) async fn record_idle_pause_failure(&self, lease_id: &LeaseId, now: SystemTime) {
        let current = match self.active_leases.get(lease_id).await {
            Ok(Some(current)) if current.status == LeaseStatus::Running => current,
            Ok(_) => return,
            Err(e) => {
                warn!(error = %e, lease_id = %lease_id, "idle pause backoff: lease lookup failed");
                return;
            }
        };
        let retry = crate::idle::IdlePauseBackoff::after_failure(current.idle_pause_retry.as_ref(), now);
        let retry_in_secs = retry.retry_at.duration_since(now).unwrap_or_default().as_secs();
        warn!(lease_id = %lease_id, failures = retry.failures, retry_in_secs, "idle pause did not take effect");
        let mut updated = current.clone();
        updated.idle_pause_retry = Some(retry);
        if let Err(e) = self.store_lease_update(&current, &updated).await {
            warn!(error = ?e, lease_id = %lease_id, "idle pause backoff: lease record write failed");
        }
    }

    /// The pause path shared by the verb and the idle evaluator. `by_idle`
    /// marks the record so the idle chain keeps governing the paused lease.
    pub(crate) async fn pause_lease(&self, req: PauseLeaseRequest, by_idle: bool) -> PauseLeaseResponse {
        let record = match self.owned_lease(&req.tenant, &req.lease_id).await {
            Ok(r) => r,
            Err(e) => return PauseLeaseResponse::Denied(e),
        };
        match record.status {
            LeaseStatus::Paused => {
                return PauseLeaseResponse::Paused {
                    reap_at: record.deadline,
                    operation_id: None,
                };
            }
            LeaseStatus::Running => {}
            _ => {
                return PauseLeaseResponse::Denied(LeaseError::LeaseNotRunning { lease_id: req.lease_id });
            }
        }

        let requirements = CapabilityRequirements {
            needs_pause_resume: CapabilityNeed::Required,
            ..Default::default()
        };
        if let Some(profile) = self.worker_registry.profile_for_provider(record.provider).await
            && !profile.satisfies(&requirements)
        {
            warn!(lease_id = %req.lease_id, provider = ?record.provider, "pause denied: provider cannot pause");
            return PauseLeaseResponse::Unsupported;
        }

        let record = match self.mark_freeze_in_flight(&record, FreezeAction::Pause).await {
            Ok(marked) => marked,
            Err(e) => {
                warn!(lease_id = %req.lease_id, "pause denied: lease record write failed");
                return PauseLeaseResponse::Denied(e);
            }
        };
        match self
            .dispatch_freeze(record.provider, &req.lease_id, FreezeAction::Pause)
            .await
        {
            Ok(()) | Err(SandboxFreezeResponse::Done) => {}
            Err(SandboxFreezeResponse::Unsupported) => {
                self.clear_freeze_in_flight(&record).await;
                return PauseLeaseResponse::Unsupported;
            }
            Err(SandboxFreezeResponse::Denied(e)) => {
                self.clear_freeze_in_flight(&record).await;
                return PauseLeaseResponse::Denied(e);
            }
        }

        let now = SystemTime::now();
        let budget = req
            .max_paused_seconds
            .unwrap_or(DEFAULT_MAX_PAUSED_SECONDS)
            .min(MAX_PAUSED_SECONDS_CEILING);
        let reap_at = now + Duration::from_secs(u64::from(budget));
        let mut updated = record.clone();
        updated.status = LeaseStatus::Paused;
        updated.freeze_in_flight = None;
        updated.paused_remaining_seconds = Some(record.deadline.duration_since(now).unwrap_or_default().as_secs());
        updated.deadline = reap_at;
        if by_idle {
            updated.auto_paused_at = Some(now);
        }
        updated.idle_pause_retry = None;
        if let Err(e) = self.store_lease_update(&record, &updated).await {
            warn!(lease_id = %req.lease_id, "pause denied: lease record write failed");
            return PauseLeaseResponse::Denied(e);
        }

        if let Err(e) = self.usage.mark_paused(&record, now).await {
            warn!(error = %e, lease_id = %req.lease_id, "usage pause stamp failed");
        }
        info!(lease_id = %req.lease_id, "lease paused");
        PauseLeaseResponse::Paused {
            reap_at,
            operation_id: None,
        }
    }

    /// Thaws a paused lease and restores the run time it had left when it was
    /// paused. Idempotent: a running lease is returned unchanged.
    /// The resume itself, without recording intent. The entity reconciler runs
    /// this for a `Running` entity whose lease is still paused.
    pub(super) async fn drive_resume_lease(&self, req: ResumeLeaseRequest) -> ResumeLeaseResponse {
        let Some(operations) = self.operations.clone() else {
            return self.resume_lease_inline(req).await;
        };
        match self
            .run_operation(
                &operations,
                &req.tenant,
                &req.lease_id,
                req.operation_id,
                OperationRequest::Resume,
            )
            .await
        {
            Ok((operation_id, OperationResult::Resumed { deadline })) => ResumeLeaseResponse::Resumed {
                deadline,
                operation_id: Some(operation_id),
            },
            Ok((_, OperationResult::Unsupported)) => ResumeLeaseResponse::Unsupported,
            Ok((_, other)) => ResumeLeaseResponse::Denied(unexpected_result(&other)),
            Err(error) => ResumeLeaseResponse::Denied(error),
        }
    }

    pub(crate) async fn resume_lease_inline(&self, req: ResumeLeaseRequest) -> ResumeLeaseResponse {
        let record = match self.owned_lease(&req.tenant, &req.lease_id).await {
            Ok(r) => r,
            Err(e) => return ResumeLeaseResponse::Denied(e),
        };
        match record.status {
            LeaseStatus::Running => {
                return ResumeLeaseResponse::Resumed {
                    deadline: record.deadline,
                    operation_id: None,
                };
            }
            LeaseStatus::Paused => {}
            _ => {
                return ResumeLeaseResponse::Denied(LeaseError::LeaseNotRunning { lease_id: req.lease_id });
            }
        }

        let record = match self.mark_freeze_in_flight(&record, FreezeAction::Resume).await {
            Ok(marked) => marked,
            Err(e) => {
                warn!(lease_id = %req.lease_id, "resume denied: lease record write failed");
                return ResumeLeaseResponse::Denied(e);
            }
        };
        match self
            .dispatch_freeze(record.provider, &req.lease_id, FreezeAction::Resume)
            .await
        {
            Ok(()) | Err(SandboxFreezeResponse::Done) => {}
            Err(SandboxFreezeResponse::Unsupported) => {
                self.clear_freeze_in_flight(&record).await;
                return ResumeLeaseResponse::Unsupported;
            }
            Err(SandboxFreezeResponse::Denied(e)) => {
                self.clear_freeze_in_flight(&record).await;
                return ResumeLeaseResponse::Denied(e);
            }
        }

        let deadline = SystemTime::now() + Duration::from_secs(record.paused_remaining_seconds.unwrap_or(0));
        let mut updated = record.clone();
        updated.status = LeaseStatus::Running;
        updated.freeze_in_flight = None;
        updated.paused_remaining_seconds = None;
        updated.auto_paused_at = None;
        updated.deadline = deadline;
        // Time spent paused is not idleness: restart the idle clock.
        if !crate::idle::idle_stages(&updated.spec).is_empty() {
            updated.last_activity_at = Some(SystemTime::now());
        }
        if let Err(e) = self.store_lease_update(&record, &updated).await {
            warn!(lease_id = %req.lease_id, "resume denied: lease record write failed");
            return ResumeLeaseResponse::Denied(e);
        }

        if let Err(e) = self.usage.mark_resumed(&record, SystemTime::now()).await {
            warn!(error = %e, lease_id = %req.lease_id, "usage resume stamp failed");
        }
        info!(lease_id = %req.lease_id, "lease resumed");
        ResumeLeaseResponse::Resumed {
            deadline,
            operation_id: None,
        }
    }

    /// Records the pause or resume about to be sent before sending it, so a
    /// broker that stops before recording the outcome leaves a marker the
    /// reconciler can act on instead of a frozen sandbox the record calls
    /// running (or the reverse).
    async fn mark_freeze_in_flight(
        &self,
        record: &LeaseRecord,
        action: FreezeAction,
    ) -> Result<LeaseRecord, LeaseError> {
        let mut marked = record.clone();
        marked.freeze_in_flight = Some(FreezeInFlight {
            action,
            started_at: SystemTime::now(),
            settle_attempts: 0,
            next_settle_at: None,
        });
        self.store_lease_update(record, &marked).await?;
        Ok(marked)
    }

    /// Drops the marker after a pause or resume the worker refused. A failed
    /// write leaves the marker for the reconciler, which settles the worker
    /// on the recorded status anyway.
    async fn clear_freeze_in_flight(&self, marked: &LeaseRecord) {
        let mut cleared = marked.clone();
        cleared.freeze_in_flight = None;
        if let Err(e) = self.store_lease_update(marked, &cleared).await {
            warn!(error = ?e, lease_id = %marked.lease_id, "failed to clear the freeze marker; the reconciler will settle it");
        }
    }

    /// Settles pauses and resumes whose broker stopped between asking the
    /// worker and recording the outcome. The record's status is what the
    /// client was last told, so the worker is driven back to it: thawed for a
    /// running lease, frozen for a paused one. Workers treat a repeat of their
    /// current state as done. A marker stays until the worker confirms; a
    /// refusal backs the next attempt off, and after
    /// [`FREEZE_SETTLE_MAX_ATTEMPTS`] refusals the lease is counted in the
    /// `sandbox_broker_unsettled_freezes` gauge instead of being asked again.
    pub async fn reconcile_stalled_freezes(&self) {
        let records = match self.active_leases.all().await {
            Ok(records) => records,
            Err(error) => {
                warn!(%error, "freeze reconciliation listing failed");
                return;
            }
        };
        let now = SystemTime::now();
        let mut unsettled = 0;
        for record in records {
            let Some(in_flight) = record.freeze_in_flight else {
                continue;
            };
            let stale = now
                .duration_since(in_flight.started_at)
                .is_ok_and(|age| age >= FREEZE_IN_FLIGHT_STALE_AFTER);
            if !stale {
                continue;
            }
            if in_flight.settle_attempts >= FREEZE_SETTLE_MAX_ATTEMPTS {
                unsettled += 1;
                continue;
            }
            if in_flight.next_settle_at.is_some_and(|at| now < at) {
                continue;
            }
            let action = match record.status {
                LeaseStatus::Running => FreezeAction::Resume,
                LeaseStatus::Paused => FreezeAction::Pause,
                // Ending leases tear the sandbox down whatever its frozen state.
                _ => continue,
            };
            match self.dispatch_freeze(record.provider, &record.lease_id, action).await {
                Ok(()) | Err(SandboxFreezeResponse::Unsupported) => {
                    info!(lease_id = %record.lease_id, thaw = action.resumes(), "stalled pause/resume settled on the recorded status");
                    self.clear_freeze_in_flight(&record).await;
                }
                Err(other) => {
                    let settle_attempts = in_flight.settle_attempts + 1;
                    let mut backed_off = record.clone();
                    backed_off.freeze_in_flight = Some(FreezeInFlight {
                        settle_attempts,
                        next_settle_at: Some(now + freeze_settle_backoff(settle_attempts)),
                        ..in_flight
                    });
                    if settle_attempts >= FREEZE_SETTLE_MAX_ATTEMPTS {
                        unsettled += 1;
                        error!(lease_id = %record.lease_id, thaw = action.resumes(), settle_attempts, reply = ?other, "stalled pause/resume not settled; giving up until an operator intervenes");
                    } else {
                        warn!(lease_id = %record.lease_id, thaw = action.resumes(), settle_attempts, reply = ?other, "stalled pause/resume not settled; backing off");
                    }
                    if let Err(e) = self.store_lease_update(&record, &backed_off).await {
                        warn!(error = ?e, lease_id = %record.lease_id, "failed to record the freeze settle attempt");
                    }
                }
            }
        }
        self.metrics.unsettled_freezes.set(unsettled);
    }
}

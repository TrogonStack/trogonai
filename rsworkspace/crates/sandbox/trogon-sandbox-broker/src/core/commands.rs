use super::*;
use crate::command_launch::{
    CancellationAuthorityClaim, CancellationCustodySupport, CommandCancellationAuthority, CommandLaunchError,
};
use crate::profile_satisfaction::ProfileSatisfies;
use trogon_sandbox_contract::{CapabilityNeed, CommandExecution, LaunchKey};
use trogon_sandbox_protocol::fit_run_command_response;

mod cancel_launch;
mod poll_kill;
mod process_control;
mod readiness;

impl BrokerCore {
    /// Runs an arbitrary command inside an already-running lease's sandbox via
    /// the owning worker. Routed the same way as checkpoint_running: broadcast
    /// to every worker for `record.provider`, and only the worker holding
    /// `record.lease_id` replies.
    pub async fn handle_run_command(&self, mut req: RunCommandRequest) -> RunCommandResponse {
        req.max_output_bytes = Some(trogon_sandbox_protocol::effective_max_output_bytes(
            req.max_output_bytes,
            self.max_command_output_bytes,
        ) as u64);
        let detached = matches!(req.execution, CommandExecution::Detached { .. });
        if req.require_launch_cancellation.is_required() && (!detached || req.launch_key.is_none()) {
            return RunCommandResponse::Denied(LeaseError::InvalidSpec {
                reason: "launch cancellation requires a detached command and launch_key".into(),
            });
        }

        let record = match self.active_leases.get(&req.lease_id).await {
            Ok(Some(r)) => r,
            Ok(None) => {
                warn!(lease_id = %req.lease_id, "run_command denied: lease not found");
                return RunCommandResponse::Denied(LeaseError::LeaseNotFound { lease_id: req.lease_id });
            }
            Err(e) => {
                warn!(error = %e, "run_command denied: active lease lookup failed");
                return RunCommandResponse::Denied(LeaseError::Backend { message: e.to_string() });
            }
        };

        if record.status != LeaseStatus::Running {
            warn!(lease_id = %req.lease_id, status = ?record.status, "run_command denied: lease is not running");
            return RunCommandResponse::Denied(self.not_running_error(&record).await);
        }

        if req.require_launch_cancellation.is_required() && !self.command_launch_cancellation_supported(&record).await {
            return RunCommandResponse::Denied(LeaseError::InvalidSpec {
                reason: "the lease has no live assigned worker with verified launch cancellation support".into(),
            });
        }

        let (interactive, process) = match &req.execution {
            CommandExecution::Detached { stdin, process, .. } => (*stdin == StdinMode::Interactive, process.as_ref()),
            CommandExecution::Attached => (false, None),
        };
        let requirements = CapabilityRequirements {
            needs_interactive_stdin: CapabilityNeed::from(interactive),
            needs_named_processes: CapabilityNeed::from(process.is_some_and(|p| p.name.is_some())),
            needs_process_readiness: CapabilityNeed::from(process.is_some_and(|p| p.readiness.is_some())),
            ..Default::default()
        };
        if requirements != CapabilityRequirements::default() && self.config.enforce_worker_registry {
            let supported = self
                .worker_registry
                .profile_for_provider(record.provider)
                .await
                .is_some_and(|profile| profile.satisfies(&requirements));
            if !supported {
                warn!(lease_id = %req.lease_id, provider = ?record.provider, ?requirements, "run_command denied: provider lacks a required exec capability");
                return RunCommandResponse::Denied(LeaseError::NoWorkerSatisfiesCapabilities { requirements });
            }
        }

        if !req.redact_values.is_empty() {
            // Fail closed whether or not the registry is enforced: a worker that
            // ignores the field would hand the secret straight back to the caller.
            let requirements = CapabilityRequirements {
                needs_output_redaction: CapabilityNeed::Required,
                ..Default::default()
            };
            let supported = self
                .worker_registry
                .profile_for_provider(record.provider)
                .await
                .is_some_and(|profile| profile.satisfies(&requirements));
            if !supported {
                warn!(lease_id = %req.lease_id, provider = ?record.provider, "run_command denied: provider does not support output redaction");
                return RunCommandResponse::Denied(LeaseError::NoWorkerSatisfiesCapabilities { requirements });
            }
        }

        match req.launch_key.clone() {
            None => self.dispatch_run_command(&record, &req, None).await.response(),
            Some(key) => self.launch_run_command_once(&record, &req, &key).await,
        }
    }

    async fn command_launch_cancellation_supported(&self, record: &LeaseRecord) -> bool {
        let (Some(worker), Some(attempt), Some(epoch)) = (
            record.worker_id.as_ref(),
            record.dispatch_attempt.as_ref(),
            record.epoch,
        ) else {
            return false;
        };
        if attempt.epoch != Some(epoch) {
            return false;
        }
        self.worker_registry
            .current_worker(record.provider, worker)
            .await
            .is_some_and(|heartbeat| {
                heartbeat.worker_id == *worker
                    && heartbeat.provider == record.provider
                    && heartbeat.profile.provider == record.provider
                    && heartbeat.supports_lease_epoch.is_supported()
                    && heartbeat.dispatch_protocol == trogon_sandbox_contract::WorkerDispatchProtocol::TargetedV1
                    && heartbeat.profile.isolation.supports_command_cancellation.is_supported()
                    && heartbeat
                        .profile
                        .isolation
                        .supports_command_launch_cancellation
                        .is_supported()
            })
    }

    fn command_verb_scope(record: &LeaseRecord) -> Option<trogon_sandbox_protocol::VerbScope> {
        let worker = record.worker_id.as_ref()?;
        let attempt = record.dispatch_attempt.as_ref()?;
        let epoch = record.epoch?;
        if attempt.epoch != Some(epoch) {
            return None;
        }
        Some(trogon_sandbox_protocol::VerbScope {
            tenant: record.tenant.clone(),
            lease_id: record.lease_id.clone(),
            worker_id: Some(worker.clone()),
            attempt_id: Some(attempt.id.clone()),
            epoch: Some(epoch),
        })
    }

    /// Dispatches `req` at most once per `launch_key`: the first call claims
    /// the key and runs the command, and every later call with the same key
    /// replays that first reply (waiting while it is still in flight)
    /// instead of running the command again.
    async fn launch_run_command_once(
        &self,
        record: &LeaseRecord,
        req: &RunCommandRequest,
        key: &LaunchKey,
    ) -> RunCommandResponse {
        let Ok(fingerprint) = crate::command_launch::fingerprint(req) else {
            return RunCommandResponse::Denied(LeaseError::InvalidSpec {
                reason: "the command request could not be fingerprinted".into(),
            });
        };
        let custody_supported =
            self.command_launches.cancellation_custody_support() == CancellationCustodySupport::Retained;
        let authority = if req.require_launch_cancellation.is_required() {
            if !custody_supported {
                return RunCommandResponse::Denied(LeaseError::InvalidSpec {
                    reason: "the command journal cannot retain strong launch custody".into(),
                });
            }
            match CommandCancellationAuthority::for_launch(record, key, &fingerprint) {
                Some(authority) => Some(authority),
                None => {
                    return RunCommandResponse::Denied(LeaseError::InvalidSpec {
                        reason: "strong launch custody requires an exact assigned native receipt".into(),
                    });
                }
            }
        } else {
            None
        };
        let dispatch_timeout = self.run_command_dispatch_timeout(record, req);
        let wait_limit = tokio::time::Instant::now() + dispatch_timeout;
        loop {
            let claim = match self.command_launches.claim(&req.lease_id, key, &fingerprint).await {
                Ok(claim) => claim,
                Err(e) => {
                    warn!(error = %e, lease_id = %req.lease_id, "run_command denied: launch key journal unavailable");
                    return RunCommandResponse::Denied(LeaseError::Backend {
                        message: format!("launch key journal unavailable: {e}"),
                    });
                }
            };
            let existing = match claim {
                Claim::Claimed => break,
                Claim::Existing(existing) => existing,
            };
            if existing.fingerprint != fingerprint {
                warn!(lease_id = %req.lease_id, "run_command denied: launch_key reused for a different command");
                return RunCommandResponse::Denied(LeaseError::InvalidSpec {
                    reason: "launch_key was already used for a different command on this lease".to_string(),
                });
            }
            if let Some(authority) = &authority {
                match self.command_launches.cancellation_authority(&req.lease_id, key).await {
                    Ok(Some(retained)) if retained == *authority => {}
                    Ok(None) if matches!(existing.state, LaunchState::Pending) => {}
                    Ok(_) => {
                        return RunCommandResponse::Denied(LeaseError::InvalidSpec {
                            reason: "the replay has no matching retained launch custody".into(),
                        });
                    }
                    Err(_) => {
                        return RunCommandResponse::Denied(LeaseError::Backend {
                            message: "retained launch custody is unavailable".into(),
                        });
                    }
                }
            }
            let now = SystemTime::now();
            match &existing.state {
                LaunchState::Done { .. } if existing.older_than(LAUNCH_KEY_REPLAY_WINDOW, now) => {
                    let _ = self
                        .command_launches
                        .replace(&req.lease_id, key, &existing, &existing.retired())
                        .await;
                    return retired_launch_reply();
                }
                LaunchState::Done { response } => return response.clone(),
                LaunchState::Retired => return retired_launch_reply(),
                // Nothing finishes a launch whose broker crashed mid-dispatch.
                // Past any timeout the first dispatch could still be inside,
                // its outcome is unknown, and that is what every retry replays.
                LaunchState::Pending if existing.older_than(dispatch_timeout + LAUNCH_KEY_LOST_GRACE, now) => {
                    let lost = CommandLaunch::new(
                        fingerprint.clone(),
                        LaunchState::Done {
                            response: lost_launch_reply(),
                        },
                    );
                    match self
                        .command_launches
                        .replace(&req.lease_id, key, &existing, &lost)
                        .await
                    {
                        Ok(true) => return lost_launch_reply(),
                        Ok(false) => continue,
                        Err(e) => {
                            return RunCommandResponse::Denied(LeaseError::Backend {
                                message: format!("launch key journal unavailable: {e}"),
                            });
                        }
                    }
                }
                LaunchState::Pending if tokio::time::Instant::now() >= wait_limit => {
                    return RunCommandResponse::Denied(LeaseError::Backend {
                        message: "the first launch with this launch_key is still in flight".to_string(),
                    });
                }
                LaunchState::Pending => tokio::time::sleep(LAUNCH_KEY_POLL_INTERVAL).await,
            }
        }

        let readiness_profile = if authority.is_some() {
            match self.command_launches.cancellation_authority(&req.lease_id, key).await {
                Ok(Some(_)) => {
                    let response = RunCommandResponse::Denied(LeaseError::Backend {
                        message: "this launch key has retained custody; its earlier outcome cannot authorize another dispatch".into(),
                    });
                    if let Err(error) = self
                        .record_launch_reply(&req.lease_id, key, fingerprint, &response)
                        .await
                    {
                        warn!(%error, lease_id = %req.lease_id, "failed to record retained launch refusal");
                    }
                    return response;
                }
                Ok(None) => {}
                Err(_) => {
                    return RunCommandResponse::Denied(LeaseError::Backend {
                        message: "retained launch custody is unavailable".into(),
                    });
                }
            }
            let profile = match record.worker_id.as_ref() {
                Some(worker) => self
                    .worker_registry
                    .current_worker(record.provider, worker)
                    .await
                    .filter(|heartbeat| {
                        heartbeat.worker_id == *worker
                            && heartbeat.provider == record.provider
                            && heartbeat.profile.provider == record.provider
                            && heartbeat.profile.isolation.supports_command_cancellation.is_supported()
                            && heartbeat
                                .profile
                                .isolation
                                .supports_command_launch_cancellation
                                .is_supported()
                    })
                    .map(|heartbeat| heartbeat.profile),
                None => None,
            };
            let ready = match profile.as_ref() {
                Some(profile) => self
                    .command_readiness(record, req.run_as_user.as_ref(), Some(profile))
                    .await
                    .is_some(),
                None => false,
            };
            if !ready {
                if self.command_launches.forget(&req.lease_id, key).await.is_err() {
                    return RunCommandResponse::Denied(LeaseError::Backend {
                        message: "the undispatched launch reply claim could not be released".into(),
                    });
                }
                return RunCommandResponse::Denied(LeaseError::InvalidSpec {
                    reason: "the exact requested command has no current verified readiness".into(),
                });
            }
            profile
        } else {
            None
        };

        let retained_exists = if let Some(authority) = &authority {
            match self
                .command_launches
                .retain_cancellation_authority(&req.lease_id, key, authority)
                .await
            {
                Ok(CancellationAuthorityClaim::New) => false,
                Ok(CancellationAuthorityClaim::Existing(_)) => true,
                Err(_) => {
                    return RunCommandResponse::Denied(LeaseError::Backend {
                        message: "strong launch custody could not be durably reserved".into(),
                    });
                }
            }
        } else if custody_supported {
            match self.command_launches.cancellation_authority(&req.lease_id, key).await {
                Ok(Some(_)) => true,
                Ok(None) => false,
                Err(_) => {
                    return RunCommandResponse::Denied(LeaseError::Backend {
                        message: "retained launch custody is unavailable".into(),
                    });
                }
            }
        } else {
            false
        };
        if retained_exists {
            let response = RunCommandResponse::Denied(LeaseError::Backend {
                message: "this launch key has retained custody; its earlier outcome cannot authorize another dispatch"
                    .into(),
            });
            if let Err(error) = self
                .record_launch_reply(&req.lease_id, key, fingerprint, &response)
                .await
            {
                warn!(%error, lease_id = %req.lease_id, "failed to record retained launch refusal");
            }
            return response;
        }

        let profile_held = match readiness_profile.as_ref() {
            Some(profile) => match record.worker_id.as_ref() {
                Some(worker) => self
                    .worker_registry
                    .current_worker(record.provider, worker)
                    .await
                    .is_some_and(|heartbeat| {
                        heartbeat.worker_id == *worker
                            && heartbeat.provider == record.provider
                            && heartbeat.profile == *profile
                            && heartbeat.supports_lease_epoch.is_supported()
                            && heartbeat.dispatch_protocol
                                == trogon_sandbox_contract::WorkerDispatchProtocol::TargetedV1
                    }),
                None => false,
            },
            None => true,
        };
        let dispatched = if profile_held {
            self.dispatch_run_command(record, req, authority.as_ref()).await
        } else {
            Dispatched::NotDelivered(RunCommandResponse::Denied(LeaseError::InvalidSpec {
                reason: "the verified command readiness profile changed before dispatch".into(),
            }))
        };
        let journaled = match &dispatched {
            // Only validation rejection or definite nondelivery proves that
            // retrying cannot duplicate a command's effects.
            Dispatched::Reply(RunCommandResponse::Denied(LeaseError::InvalidSpec { .. }))
            | Dispatched::NotDelivered(_) => self.command_launches.forget(&req.lease_id, key).await,
            // Worker errors can follow execution, and transport failures leave
            // its fate unknown. Both must retain the launch identity.
            Dispatched::Reply(response) | Dispatched::Unknown(response) => {
                self.record_launch_reply(&req.lease_id, key, fingerprint, response)
                    .await
            }
        };
        if let Err(e) = journaled {
            warn!(error = %e, lease_id = %req.lease_id, "run_command launch key journal update failed");
        }
        dispatched.response()
    }

    /// Stores the reply a launch key replays. A reply too large for the
    /// journal is stored trimmed, and failing that as a short marker, because
    /// a key left pending would answer every retry with "still in flight"
    /// until the journal's age limit instead of saying the command ran.
    async fn record_launch_reply(
        &self,
        lease: &LeaseId,
        key: &LaunchKey,
        fingerprint: String,
        response: &RunCommandResponse,
    ) -> Result<(), CommandLaunchError> {
        if let Err(e) = self
            .store_launch_reply(lease, key, &fingerprint, response.clone())
            .await
        {
            warn!(error = %e, lease_id = %lease, "run_command launch reply did not fit the journal; storing it trimmed");
            let trimmed = fit_run_command_response(response.clone(), LAUNCH_JOURNAL_MAX_REPLY_BYTES);
            if self.store_launch_reply(lease, key, &fingerprint, trimmed).await.is_ok() {
                return Ok(());
            }
            return self
                .store_launch_reply(lease, key, &fingerprint, unkept_launch_reply())
                .await;
        }
        Ok(())
    }

    /// Records one candidate reply for a claimed key and schedules its retirement.
    async fn store_launch_reply(
        &self,
        lease: &LeaseId,
        key: &LaunchKey,
        fingerprint: &str,
        response: RunCommandResponse,
    ) -> Result<(), CommandLaunchError> {
        let launch = CommandLaunch::new(fingerprint, LaunchState::Done { response });
        self.command_launches.finish(lease, key, &launch).await?;
        self.retire_launch_after_replay_window(lease, key, launch);
        Ok(())
    }

    /// Drops the stored reply once a retry of a lost reply has had its
    /// chance, so command output does not sit in the journal for the rest of
    /// the lease. Best effort: a broker that stops first leaves it to the
    /// next retry, the lease's end or the journal's age limit.
    fn retire_launch_after_replay_window(&self, lease: &LeaseId, key: &LaunchKey, launch: CommandLaunch) {
        let store = Arc::clone(&self.command_launches);
        let lease = lease.clone();
        let key = key.clone();
        tokio::spawn(async move {
            tokio::time::sleep(LAUNCH_KEY_REPLAY_WINDOW).await;
            if let Err(e) = store.replace(&lease, &key, &launch, &launch.retired()).await {
                warn!(error = %e, lease_id = %lease, "failed to drop a replayed launch reply");
            }
        });
    }

    fn run_command_dispatch_timeout(&self, record: &LeaseRecord, req: &RunCommandRequest) -> Duration {
        if matches!(req.execution, CommandExecution::Detached { .. }) {
            detached_start_timeout(record.deadline, SystemTime::now())
        } else {
            foreground_run_command_timeout(req.timeout_seconds, record.deadline, SystemTime::now())
        }
    }

    async fn dispatch_run_command(
        &self,
        record: &LeaseRecord,
        req: &RunCommandRequest,
        authority: Option<&CommandCancellationAuthority>,
    ) -> Dispatched {
        let lease_id = &req.lease_id;
        let body = match serde_json::to_vec(req) {
            Ok(b) => Bytes::from(b),
            Err(e) => {
                return Dispatched::Reply(RunCommandResponse::Denied(LeaseError::Backend {
                    message: e.to_string(),
                }));
            }
        };

        if let Some(authority) = authority {
            match self.active_leases.get(lease_id).await {
                Ok(Some(current))
                    if current.status == LeaseStatus::Running
                        && current.deadline > SystemTime::now()
                        && current.freeze_in_flight.is_none()
                        && authority.matches_record(&current) => {}
                Ok(_) => {
                    return Dispatched::NotDelivered(RunCommandResponse::Denied(LeaseError::InvalidSpec {
                        reason: "the reserved command assignment changed before dispatch".into(),
                    }));
                }
                Err(_) => {
                    return Dispatched::NotDelivered(RunCommandResponse::Denied(LeaseError::Backend {
                        message: "the reserved command assignment is unavailable".into(),
                    }));
                }
            }
        }
        self.touch_activity(record, false).await;
        let subject = trogon_sandbox_protocol::subjects::provider_run_command(record.provider, lease_id);
        let timeout = self.run_command_dispatch_timeout(record, req);
        let sent = match (authority, self.dispatch_signer.as_ref()) {
            (Some(authority), Some(signer)) => {
                let auth = match signer.sign_verb(&subject, &body, &authority.scope()) {
                    Ok(auth) => auth,
                    Err(e) => {
                        warn!(error = %e, lease_id = %lease_id, "run_command denied: dispatch signing failed");
                        return Dispatched::NotDelivered(RunCommandResponse::Denied(LeaseError::Backend {
                            message: e.to_string(),
                        }));
                    }
                };
                self.bus.request_with_auth(&subject, &auth, body, Some(timeout)).await
            }
            _ => self.bus.request_with_timeout(&subject, body, timeout).await,
        };
        let reply = match sent {
            Ok(r) => r,
            Err(e) => {
                warn!(error = %e, lease_id = %lease_id, "run_command denied: worker dispatch failed");
                let denied = RunCommandResponse::Denied(LeaseError::Backend { message: e.to_string() });
                return if is_no_responders(&e) {
                    Dispatched::NotDelivered(denied)
                } else {
                    Dispatched::Unknown(denied)
                };
            }
        };

        // A foreground command can outlive the idle window; count its end as activity.
        self.touch_activity(record, true).await;

        match serde_json::from_slice::<RunCommandResponse>(&reply) {
            Ok(resp) => {
                match &resp {
                    RunCommandResponse::Completed(out) => {
                        self.count_usage(
                            record,
                            LeaseUsageCounters {
                                commands_run: 1,
                                command_output_bytes: (out.stdout.len() + out.stderr.len()) as u64,
                                ..Default::default()
                            },
                        )
                        .await;
                    }
                    RunCommandResponse::Started(_) => {
                        self.count_usage(
                            record,
                            LeaseUsageCounters {
                                commands_run: 1,
                                ..Default::default()
                            },
                        )
                        .await;
                    }
                    RunCommandResponse::Denied(_) => {}
                }
                Dispatched::Reply(resp)
            }
            Err(e) => Dispatched::Unknown(RunCommandResponse::Denied(LeaseError::Backend {
                message: format!("run_command reply decode failed: {e}"),
            })),
        }
    }

    /// The provider of a running lease, or the denial naming why it has none.
    async fn running_lease_provider(&self, lease_id: &LeaseId) -> Result<ProviderKind, LeaseError> {
        match self.active_leases.get(lease_id).await {
            Ok(Some(r)) if r.status == LeaseStatus::Running => Ok(r.provider),
            Ok(Some(r)) => Err(self.not_running_error(&r).await),
            Ok(None) => Err(LeaseError::LeaseNotFound {
                lease_id: lease_id.clone(),
            }),
            Err(e) => Err(LeaseError::Backend { message: e.to_string() }),
        }
    }

    /// Whether `provider` declares the capabilities in `requirements`. Always
    /// true when the worker registry is not enforced.
    pub(super) async fn provider_satisfies(
        &self,
        provider: ProviderKind,
        requirements: &CapabilityRequirements,
    ) -> bool {
        !self.config.enforce_worker_registry
            || self
                .worker_registry
                .profile_for_provider(provider)
                .await
                .is_some_and(|profile| profile.satisfies(requirements))
    }
}

//! Suspend and wake: release a lease's host resources while keeping a durable
//! artifact, then bring the same lease id back.
//!
//! A suspend walks `Running | Paused -> Suspending -> Suspended`. The
//! `Suspending` record, written before the worker is asked, is the marker a
//! restart finds: the operation driver re-drives the suspend, and workers treat
//! a repeat of the same generation as success. A wake walks `Suspended ->
//! Waking -> Running`; the compare-and-swap into `Waking` is the single-flight
//! claim, so a concurrent wake is told to retry instead of reaching the worker.
//! A refused suspend rolls the record back, so the lease stays as it was.

use crate::constants::{DEFAULT_MAX_SUSPENDED_SECONDS, MAX_SUSPENDED_SECONDS_CEILING, MAX_WAKE_FAILURES};
use serde::{Serialize, de::DeserializeOwned};
use trogon_sandbox_contract::{
    DormancyRecord, SandboxSuspendRequest, SandboxSuspendResponse, SandboxWakeRequest, SandboxWakeResponse,
    SuspendLeaseRequest, SuspendLeaseResponse, SuspendLevel, SuspendRequestLevel, WakeLeaseRequest, WakeLeaseResponse,
};

use super::constants::WAKE_RETRY_AFTER_MS;
use super::*;

impl BrokerCore {
    /// Suspends a lease to the level its provider offers (or the one asked
    /// for), keeping its id. Idempotent.
    pub async fn handle_suspend_lease(&self, req: SuspendLeaseRequest) -> SuspendLeaseResponse {
        let Some(operations) = self.operations.clone() else {
            return self.suspend_lease_inline(req, false).await;
        };
        match self
            .run_operation(
                &operations,
                &req.tenant,
                &req.lease_id,
                req.operation_id,
                OperationRequest::Suspend {
                    level: req.level,
                    max_suspended_seconds: req.max_suspended_seconds,
                },
            )
            .await
        {
            Ok((operation_id, OperationResult::Suspended { level, reap_at })) => SuspendLeaseResponse::Suspended {
                level,
                reap_at,
                operation_id: Some(operation_id),
            },
            Ok((_, other)) => SuspendLeaseResponse::Denied(unexpected_result(&other)),
            Err(error) => SuspendLeaseResponse::Denied(error),
        }
    }

    /// Wakes a suspended lease, restoring the run time it had left. Idempotent:
    /// a running lease is returned unchanged.
    pub async fn handle_wake_lease(&self, req: WakeLeaseRequest) -> WakeLeaseResponse {
        let Some(operations) = self.operations.clone() else {
            return self.wake_lease_inline(req, false).await;
        };
        match self
            .run_operation(
                &operations,
                &req.tenant,
                &req.lease_id,
                req.operation_id,
                OperationRequest::Wake,
            )
            .await
        {
            Ok((operation_id, OperationResult::Woken { deadline })) => WakeLeaseResponse::Woken {
                deadline,
                operation_id: Some(operation_id),
            },
            Ok((_, other)) => WakeLeaseResponse::Denied(unexpected_result(&other)),
            Err(error) => WakeLeaseResponse::Denied(error),
        }
    }

    /// Suspends without recording an operation. `redrive` is set by the
    /// operation driver when it picks up a suspend a restart interrupted, which
    /// is the only caller allowed to continue a lease already `Suspending`.
    pub(crate) async fn suspend_lease_inline(&self, req: SuspendLeaseRequest, redrive: bool) -> SuspendLeaseResponse {
        let record = match self.owned_lease(&req.tenant, &req.lease_id).await {
            Ok(r) => r,
            Err(e) => return SuspendLeaseResponse::Denied(e),
        };
        let marked = match record.status {
            LeaseStatus::Suspended => {
                return match &record.dormancy {
                    Some(dormancy) => SuspendLeaseResponse::Suspended {
                        level: dormancy.level,
                        reap_at: record.deadline,
                        operation_id: None,
                    },
                    None => SuspendLeaseResponse::Denied(backend("suspended lease has no dormancy record")),
                };
            }
            LeaseStatus::Running | LeaseStatus::Paused => match self.begin_suspend(&record, &req).await {
                Ok(marked) => marked,
                Err(e) => return SuspendLeaseResponse::Denied(e),
            },
            LeaseStatus::Suspending if redrive => record,
            _ => {
                return SuspendLeaseResponse::Denied(LeaseError::LeaseNotRunning { lease_id: req.lease_id });
            }
        };
        let Some(dormancy) = marked.dormancy.clone() else {
            return SuspendLeaseResponse::Denied(backend("suspending lease has no dormancy record"));
        };

        let request = SandboxSuspendRequest {
            lease_id: req.lease_id.clone(),
            generation: dormancy.generation,
            level: dormancy.level,
        };
        let reply: SandboxSuspendResponse = match self
            .dispatch_dormancy(subjects::provider_suspend(marked.provider, &req.lease_id), &request)
            .await
        {
            Ok(reply) => reply,
            Err(e) => {
                // The worker may have suspended; the record stays `Suspending`
                // for the operation driver to finish.
                warn!(lease_id = %req.lease_id, error = %e, "suspend dispatch failed; left suspending");
                return SuspendLeaseResponse::Denied(e);
            }
        };
        let (artifact, artifact_bytes) = match reply {
            SandboxSuspendResponse::Done {
                artifact,
                artifact_bytes,
            } => (artifact, artifact_bytes),
            SandboxSuspendResponse::Unsupported => {
                self.rollback_suspend(&marked).await;
                return SuspendLeaseResponse::Denied(LeaseError::SuspendUnsupported {
                    lease_id: req.lease_id,
                    requested: req.level,
                });
            }
            SandboxSuspendResponse::Denied(e) => {
                self.rollback_suspend(&marked).await;
                return SuspendLeaseResponse::Denied(e);
            }
        };

        let now = SystemTime::now();
        let budget = req
            .max_suspended_seconds
            .unwrap_or(DEFAULT_MAX_SUSPENDED_SECONDS)
            .min(MAX_SUSPENDED_SECONDS_CEILING);
        let reap_at = now + Duration::from_secs(u64::from(budget));
        let mut updated = marked.clone();
        updated.status = LeaseStatus::Suspended;
        updated.deadline = reap_at;
        updated.paused_remaining_seconds = None;
        updated.auto_paused_at = None;
        updated.idle_pause_retry = None;
        if let Some(record) = updated.dormancy.as_mut() {
            record.artifact = artifact;
            record.artifact_bytes = artifact_bytes;
        }
        if let Err(e) = self.store_lease_update(&marked, &updated).await {
            warn!(lease_id = %req.lease_id, "suspend not recorded: lease record write failed");
            return SuspendLeaseResponse::Denied(e);
        }
        if let Err(e) = self.usage.mark_suspended(&marked, now, artifact_bytes).await {
            warn!(error = %e, lease_id = %req.lease_id, "usage suspend stamp failed");
        }
        info!(lease_id = %req.lease_id, level = dormancy.level.as_str(), "lease suspended");
        SuspendLeaseResponse::Suspended {
            level: dormancy.level,
            reap_at,
            operation_id: None,
        }
    }

    /// Checks the request against the lease and its provider, then records
    /// `Suspending` before the worker is asked. The record keeps the deadline
    /// and any parked pause time, so a rollback restores the lease exactly.
    async fn begin_suspend(&self, record: &LeaseRecord, req: &SuspendLeaseRequest) -> Result<LeaseRecord, LeaseError> {
        if !record.spec.retention().snapshots.is_allowed() {
            // A memory artifact can hold secrets, so both levels need captures allowed.
            return Err(LeaseError::SnapshotsDisabled {
                lease_id: req.lease_id.clone(),
            });
        }
        let level = self
            .worker_registry
            .profile_for_provider(record.provider)
            .await
            .and_then(|profile| profile.lifecycle.dormancy.resolve(req.level))
            .ok_or_else(|| {
                warn!(lease_id = %req.lease_id, provider = ?record.provider, "suspend denied: provider cannot suspend to the requested level");
                LeaseError::SuspendUnsupported {
                    lease_id: req.lease_id.clone(),
                    requested: req.level,
                }
            })?;

        let now = SystemTime::now();
        let run_remaining_seconds = record
            .paused_remaining_seconds
            .unwrap_or_else(|| record.deadline.duration_since(now).unwrap_or_default().as_secs());
        let mut marked = record.clone();
        marked.status = LeaseStatus::Suspending;
        marked.dormancy = Some(DormancyRecord {
            level,
            generation: record.dormancy.as_ref().map_or(1, |d| d.generation + 1),
            artifact: None,
            artifact_bytes: None,
            suspended_at: now,
            run_remaining_seconds,
            wake_failures: 0,
            woken_at: None,
        });
        self.store_lease_update(record, &marked).await?;
        Ok(marked)
    }

    /// Puts a lease whose suspend the worker refused back where it was. A
    /// failed write leaves `Suspending`, which a retry or the deadline settles.
    async fn rollback_suspend(&self, marked: &LeaseRecord) {
        let now = SystemTime::now();
        let mut restored = marked.clone();
        restored.status = if marked.paused_remaining_seconds.is_some() {
            LeaseStatus::Paused
        } else {
            LeaseStatus::Running
        };
        // The dormancy ended without a suspend; the generation stays so the
        // next attempt does not reuse it.
        if let Some(dormancy) = restored.dormancy.as_mut() {
            dormancy.woken_at = Some(now);
        }
        if let Err(e) = self.store_lease_update(marked, &restored).await {
            warn!(error = ?e, lease_id = %marked.lease_id, "failed to roll back a refused suspend");
        }
    }

    pub(crate) async fn wake_lease_inline(&self, req: WakeLeaseRequest, redrive: bool) -> WakeLeaseResponse {
        let record = match self.owned_lease(&req.tenant, &req.lease_id).await {
            Ok(r) => r,
            Err(e) => return WakeLeaseResponse::Denied(e),
        };
        let waking = match record.status {
            LeaseStatus::Running => {
                return WakeLeaseResponse::Woken {
                    deadline: record.deadline,
                    operation_id: None,
                };
            }
            LeaseStatus::Suspended => {
                let mut claimed = record.clone();
                claimed.status = LeaseStatus::Waking;
                if let Err(e) = self.store_lease_update(&record, &claimed).await {
                    // Losing the claim means another wake got there first.
                    return match self.owned_lease(&req.tenant, &req.lease_id).await {
                        Ok(now) if now.status == LeaseStatus::Waking => self.waking_hint(&now),
                        Ok(now) if now.status == LeaseStatus::Running => WakeLeaseResponse::Woken {
                            deadline: now.deadline,
                            operation_id: None,
                        },
                        _ => WakeLeaseResponse::Denied(e),
                    };
                }
                claimed
            }
            LeaseStatus::Waking if redrive => record,
            LeaseStatus::Waking => return self.waking_hint(&record),
            _ => {
                return WakeLeaseResponse::Denied(LeaseError::LeaseNotRunning { lease_id: req.lease_id });
            }
        };
        let Some(dormancy) = waking.dormancy.clone() else {
            return WakeLeaseResponse::Denied(backend("waking lease has no dormancy record"));
        };

        let request = SandboxWakeRequest {
            lease_id: req.lease_id.clone(),
            generation: dormancy.generation,
            level: dormancy.level,
            artifact: dormancy.artifact.clone(),
        };
        let reply: SandboxWakeResponse = match self
            .dispatch_dormancy(subjects::provider_wake(waking.provider, &req.lease_id), &request)
            .await
        {
            Ok(reply) => reply,
            Err(e) => {
                // The worker may have woken; the record stays `Waking` for the
                // operation driver to finish.
                warn!(lease_id = %req.lease_id, error = %e, "wake dispatch failed; left waking");
                return WakeLeaseResponse::Denied(e);
            }
        };
        match reply {
            SandboxWakeResponse::Done => {}
            SandboxWakeResponse::Unsupported => {
                return WakeLeaseResponse::Denied(
                    self.fail_wake(
                        &waking,
                        LeaseError::SuspendUnsupported {
                            lease_id: req.lease_id,
                            requested: match dormancy.level {
                                SuspendLevel::Memory => trogon_sandbox_contract::SuspendRequestLevel::Memory,
                                SuspendLevel::Disk => trogon_sandbox_contract::SuspendRequestLevel::Disk,
                            },
                        },
                    )
                    .await,
                );
            }
            SandboxWakeResponse::Denied(e) => {
                return WakeLeaseResponse::Denied(self.fail_wake(&waking, e).await);
            }
        }

        let now = SystemTime::now();
        let deadline = now + Duration::from_secs(dormancy.run_remaining_seconds);
        let mut updated = waking.clone();
        updated.status = LeaseStatus::Running;
        updated.deadline = deadline;
        if let Some(record) = updated.dormancy.as_mut() {
            record.woken_at = Some(now);
            record.wake_failures = 0;
            record.artifact = None;
            record.artifact_bytes = None;
        }
        // Time spent suspended is not idleness: restart the idle clock.
        if !crate::idle::idle_stages(&updated.spec).is_empty() {
            updated.last_activity_at = Some(now);
        }
        if let Err(e) = self.store_lease_update(&waking, &updated).await {
            warn!(lease_id = %req.lease_id, "wake not recorded: lease record write failed");
            return WakeLeaseResponse::Denied(e);
        }
        if let Err(e) = self.usage.mark_woken(&waking, now).await {
            warn!(error = %e, lease_id = %req.lease_id, "usage wake stamp failed");
        }
        info!(lease_id = %req.lease_id, "lease woken");
        WakeLeaseResponse::Woken {
            deadline,
            operation_id: None,
        }
    }

    fn waking_hint(&self, record: &LeaseRecord) -> WakeLeaseResponse {
        WakeLeaseResponse::Denied(Self::waking_error(record))
    }

    fn waking_error(record: &LeaseRecord) -> LeaseError {
        LeaseError::LeaseWaking {
            lease_id: record.lease_id.clone(),
            retry_after_ms: WAKE_RETRY_AFTER_MS,
        }
    }

    /// Binds the core's own handle so a verb that meets a suspended lease can
    /// start the wake in the background. Call once after the core is shared.
    pub fn bind_self(self: &Arc<Self>) {
        let _ = self.self_handle.set(Arc::downgrade(self));
    }

    /// Whether the lease's provider can suspend it, as the idle chain asks.
    pub(crate) async fn lease_can_suspend(&self, record: &LeaseRecord) -> bool {
        record.spec.retention().snapshots.is_allowed()
            && self
                .worker_registry
                .profile_for_provider(record.provider)
                .await
                .is_some_and(|profile| profile.lifecycle.dormancy.resolve(SuspendRequestLevel::Auto).is_some())
    }

    /// The answer for a verb that needs a running lease and found `record`
    /// in another state. A suspended lease is woken (single flight) and the
    /// caller is told to retry after the usual delay; a waking one gets the
    /// same answer. Anything else is `LeaseNotRunning`.
    pub(super) async fn not_running_error(&self, record: &LeaseRecord) -> LeaseError {
        match record.status {
            LeaseStatus::Waking => Self::waking_error(record),
            LeaseStatus::Suspended if self.wake_on_access(record).await => Self::waking_error(record),
            _ => LeaseError::LeaseNotRunning {
                lease_id: record.lease_id.clone(),
            },
        }
    }

    /// Starts waking a suspended lease without waiting for it. Returns whether
    /// a wake is now under way (started here or already recorded).
    ///
    /// With the operation store the wake is a durable operation whose id is
    /// fixed by the lease, its dormancy generation and its failed wakes so far,
    /// so every concurrent access joins the same record and a crash is
    /// re-driven. Without the store the wake runs as a detached task and the
    /// compare-and-swap into `Waking` is the single flight.
    async fn wake_on_access(&self, record: &LeaseRecord) -> bool {
        let Some(core) = self.self_handle.get().and_then(std::sync::Weak::upgrade) else {
            return false;
        };
        let tenant = record.tenant.clone();
        let lease_id = record.lease_id.clone();
        let Some(operations) = self.operations.clone() else {
            tokio::spawn(async move {
                core.wake_lease_inline(
                    WakeLeaseRequest {
                        tenant,
                        lease_id,
                        operation_id: None,
                    },
                    false,
                )
                .await;
            });
            return true;
        };
        let (generation, failures) = record
            .dormancy
            .as_ref()
            .map_or((0, 0), |d| (d.generation, d.wake_failures));
        let Ok(id) = OperationId::new(format!("wake-{}-{generation}-{failures}", lease_id.as_str())) else {
            return false;
        };
        match operations
            .begin(&tenant, &id, &lease_id, OperationRequest::Wake, SystemTime::now())
            .await
        {
            Ok(crate::operations::Begun::Created(_)) => {
                info!(lease_id = %lease_id, "wake on access started");
                tokio::spawn(async move {
                    if let Err(error) = core.drive_operation(&operations, &tenant, &id, false).await {
                        warn!(%error, operation_id = %id, "wake on access failed to run");
                    }
                });
                true
            }
            Ok(_) => true,
            Err(error) => {
                warn!(%error, lease_id = %lease_id, "wake on access could not be recorded");
                false
            }
        }
    }

    /// Returns a lease whose wake the worker refused to `Suspended`, counting
    /// the failure. A lost artifact, or [`MAX_WAKE_FAILURES`] failures in a
    /// row, marks it `Failed` instead; the artifact is kept for diagnosis until
    /// the suspended budget runs out. Returns the error to report.
    async fn fail_wake(&self, waking: &LeaseRecord, error: LeaseError) -> LeaseError {
        let mut updated = waking.clone();
        let mut failures = 0;
        if let Some(dormancy) = updated.dormancy.as_mut() {
            dormancy.wake_failures += 1;
            failures = dormancy.wake_failures;
        }
        let lost = matches!(error, LeaseError::SuspendArtifactLost { .. });
        updated.status = if lost || failures >= MAX_WAKE_FAILURES {
            error!(lease_id = %waking.lease_id, failures, lost, "wake failed for good; lease marked failed");
            LeaseStatus::Failed
        } else {
            LeaseStatus::Suspended
        };
        if let Err(e) = self.store_lease_update(waking, &updated).await {
            warn!(error = ?e, lease_id = %waking.lease_id, "failed to record a failed wake");
        }
        error
    }

    async fn dispatch_dormancy<Q: Serialize, R: DeserializeOwned>(
        &self,
        subject: String,
        request: &Q,
    ) -> Result<R, LeaseError> {
        let body = serde_json::to_vec(request).map(Bytes::from).map_err(backend)?;
        let reply = self.bus.request(&subject, body).await.map_err(backend)?;
        serde_json::from_slice(&reply).map_err(|e| backend(format!("dormancy reply decode failed: {e}")))
    }
}

//! Converges persistent entities toward the state their record asks for.
//!
//! The entity record carries the intent: `Stopping` asks for the run to end,
//! `Paused` asks for a paused lease, `Deleting` asks for the entity to be
//! gone. The lease store carries what is observed. This pass compares the two
//! for every entity and drives the existing operations until they agree: a
//! stop releases the lease, a pause runs the durable pause operation, a delete
//! finishes the purge, and a `Running` entity over a lease a user left frozen
//! is resumed. The stop, pause, resume and delete verbs write that intent
//! before they act (see `entity_intent`), so work a restart interrupted is
//! finished here. It never invents intent: `Starting` and `Running` are left to
//! the repair sweep, and a lease the idle evaluator paused is not resumed.
//!
//! It runs only when the entity store is authoritative (`on`). In `shadow`
//! the legacy stores decide and the entity merely follows, so acting on its
//! record could fight them.
//!
//! A pass is safe to repeat. Pause uses an operation id derived from the
//! entity and its record revision, so a second pass over the same record
//! replays the stored outcome instead of pausing twice.

use trogon_sandbox_contract::SandboxEntityRecord;
use trogon_sandbox_contract::{DeleteEntityRequest, DeleteEntityResponse, EntityRef, EntityState};
use trogon_sandbox_contract::{
    LeaseStatus, OperationId, PauseLeaseRequest, PauseLeaseResponse, ResumeLeaseRequest, ResumeLeaseResponse,
};

use super::*;

/// What one [`BrokerCore::reconcile_entities`] pass did.
#[derive(Debug, Default, Clone, Copy, Eq, PartialEq)]
pub struct ReconcileReport {
    /// Runs ended because the lease the entity named is gone.
    pub runs_ended: usize,
    /// Leases released to satisfy a stop.
    pub stops_driven: usize,
    /// Leases paused to satisfy a pause.
    pub pauses_driven: usize,
    /// Leases resumed to satisfy a resume recorded as the running state.
    pub resumes_driven: usize,
    /// Deletes carried through to the end.
    pub deletes_finished: usize,
    /// Steps that were attempted and did not take; the next pass tries again.
    pub failures: usize,
}

/// The bound lease as the lease store reports it.
#[derive(Debug, Clone, Eq, PartialEq)]
pub(super) enum Observed {
    /// No record, a released one, or one owned by another tenant.
    Gone,
    Live {
        status: LeaseStatus,
        /// A pause or resume is mid-flight; the freeze reconciler owns it.
        freezing: bool,
        /// The idle evaluator paused it, not a user: not the entity's call.
        auto_paused: bool,
    },
}

/// The one step an entity calls for.
#[derive(Debug, Clone, Eq, PartialEq)]
pub(super) enum Step {
    Nothing,
    EndRun(LeaseId),
    ReleaseLease(LeaseId),
    PauseLease(LeaseId),
    ResumeLease(LeaseId),
    FinishDelete,
}

/// Decides the step from the entity's intent and the observed lease.
pub(super) fn plan_step(state: &EntityState, observed: &Observed) -> Step {
    match (state, observed) {
        (EntityState::Deleting, _) => Step::FinishDelete,
        (EntityState::Stopping { lease } | EntityState::Paused { lease }, Observed::Gone) => {
            Step::EndRun(lease.clone())
        }
        (EntityState::Stopping { lease }, Observed::Live { freezing: false, .. }) => Step::ReleaseLease(lease.clone()),
        (
            EntityState::Paused { lease },
            Observed::Live {
                status: LeaseStatus::Running,
                freezing: false,
                ..
            },
        ) => Step::PauseLease(lease.clone()),
        // A resume was recorded and the lease is still frozen: the broker
        // died between the two. An idle pause leaves the entity `Running` too,
        // which is why a lease paused by the idle evaluator is excluded.
        (
            EntityState::Running { lease },
            Observed::Live {
                status: LeaseStatus::Paused,
                freezing: false,
                auto_paused: false,
            },
        ) => Step::ResumeLease(lease.clone()),
        _ => Step::Nothing,
    }
}

fn pause_operation_id(record: &SandboxEntityRecord, revision: u64) -> Option<OperationId> {
    OperationId::new(format!("rc-{}-{revision}-pause", record.entity_id)).ok()
}

fn resume_operation_id(record: &SandboxEntityRecord, revision: u64) -> Option<OperationId> {
    OperationId::new(format!("rc-{}-{revision}-resume", record.entity_id)).ok()
}

impl BrokerCore {
    /// Runs the entity repair sweep every `tick` until `cancel` fires. With
    /// `reconcile` set, each tick also runs [`Self::reconcile_entities`],
    /// which does nothing unless the entity store is authoritative.
    pub fn spawn_entity_sweeper(
        self: Arc<Self>,
        tick: Duration,
        reconcile: bool,
        cancel: tokio_util::sync::CancellationToken,
    ) {
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(tick);
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                tokio::select! {
                    _ = cancel.cancelled() => break,
                    _ = interval.tick() => {
                        let repaired = self.sweep_entities().await;
                        if repaired != Default::default() {
                            info!(?repaired, "sandbox entity repair sweep changed state");
                        }
                        if reconcile {
                            let reconciled = self.reconcile_entities().await;
                            if reconciled != Default::default() {
                                info!(?reconciled, "sandbox entity reconcile changed state");
                            }
                        }
                    }
                }
            }
        });
    }

    /// One reconcile pass over every entity. A no-op unless the entity store
    /// is authoritative.
    pub async fn reconcile_entities(&self) -> ReconcileReport {
        let mut report = ReconcileReport::default();
        let Some(shadow) = self.entity.as_ref().filter(|s| s.is_authoritative()) else {
            return report;
        };
        let entities = match shadow.store().list_all().await {
            Ok(entities) => entities,
            Err(error) => {
                warn!(error = %error, "entity reconcile could not list entities");
                return report;
            }
        };
        for found in entities {
            let record = &found.record;
            let observed = match self.observe_bound_lease(record).await {
                Some(observed) => observed,
                None => {
                    report.failures += 1;
                    continue;
                }
            };
            match plan_step(&record.state, &observed) {
                Step::Nothing => {}
                Step::EndRun(lease) => {
                    if shadow.end_run(record, &lease, "reconcile").await {
                        report.runs_ended += 1;
                    } else {
                        report.failures += 1;
                    }
                }
                Step::ReleaseLease(lease) => {
                    let released = self
                        .release_lease(lease, CancelIntent::Release, LeaseReleasedReason::Cancelled)
                        .await;
                    match released {
                        Ok(()) => report.stops_driven += 1,
                        Err(error) => {
                            warn!(error = %error, entity_id = %record.entity_id, "entity reconcile could not release the lease");
                            report.failures += 1;
                        }
                    }
                }
                Step::PauseLease(lease) => {
                    let response = self
                        .drive_pause_lease(PauseLeaseRequest {
                            tenant: record.tenant.clone(),
                            lease_id: lease,
                            max_paused_seconds: None,
                            operation_id: pause_operation_id(record, found.revision),
                        })
                        .await;
                    match response {
                        PauseLeaseResponse::Paused { .. } => report.pauses_driven += 1,
                        PauseLeaseResponse::Unsupported => {
                            warn!(entity_id = %record.entity_id, "entity reconcile: the provider cannot pause");
                            report.failures += 1;
                        }
                        PauseLeaseResponse::Denied(error) => {
                            warn!(error = %error, entity_id = %record.entity_id, "entity reconcile could not pause the lease");
                            report.failures += 1;
                        }
                    }
                }
                Step::ResumeLease(lease) => {
                    let response = self
                        .drive_resume_lease(ResumeLeaseRequest {
                            tenant: record.tenant.clone(),
                            lease_id: lease,
                            operation_id: resume_operation_id(record, found.revision),
                        })
                        .await;
                    match response {
                        ResumeLeaseResponse::Resumed { .. } => report.resumes_driven += 1,
                        ResumeLeaseResponse::Unsupported => {
                            warn!(entity_id = %record.entity_id, "entity reconcile: the provider cannot resume");
                            report.failures += 1;
                        }
                        ResumeLeaseResponse::Denied(error) => {
                            warn!(error = %error, entity_id = %record.entity_id, "entity reconcile could not resume the lease");
                            report.failures += 1;
                        }
                    }
                }
                Step::FinishDelete => {
                    let response = self
                        .handle_delete_entity(DeleteEntityRequest {
                            tenant: record.tenant.clone(),
                            entity: EntityRef::Id(record.entity_id.clone()),
                        })
                        .await;
                    match response {
                        DeleteEntityResponse::Deleted | DeleteEntityResponse::NotFound => report.deletes_finished += 1,
                        DeleteEntityResponse::Denied(error) => {
                            warn!(error = %error, entity_id = %record.entity_id, "entity reconcile could not finish a delete");
                            report.failures += 1;
                        }
                    }
                }
            }
        }
        report
    }

    /// The lease `record` is bound to, as observed. `None` when the lease
    /// store could not be read, so the entity is skipped this pass.
    async fn observe_bound_lease(&self, record: &SandboxEntityRecord) -> Option<Observed> {
        let Some(lease) = record.state.lease() else {
            return Some(Observed::Gone);
        };
        match self.active_leases.get(lease).await {
            Ok(Some(found)) if found.tenant == record.tenant => {
                if found.status == LeaseStatus::Released {
                    Some(Observed::Gone)
                } else {
                    Some(Observed::Live {
                        status: found.status,
                        freezing: found.freeze_in_flight.is_some(),
                        auto_paused: found.auto_paused_at.is_some(),
                    })
                }
            }
            Ok(_) => Some(Observed::Gone),
            Err(error) => {
                warn!(error = %error, "entity reconcile could not read a lease");
                None
            }
        }
    }
}

#[cfg(test)]
mod tests;

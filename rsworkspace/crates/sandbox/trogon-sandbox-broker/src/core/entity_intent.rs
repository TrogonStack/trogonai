//! Desired state writers on the stop, pause, resume and delete verbs.
//!
//! Each verb records what it wants on the entity before it acts, in `shadow`
//! and `on` alike. The record is what survives a broker restart mid-operation:
//! in `on` mode the entity reconciler reads it and finishes the work through
//! the same durable operations the verb uses. In `shadow` the intent is only
//! recorded; the reconciler does not run, so the legacy stores still decide.
//!
//! A write failure fails the verb in `on` mode (the entity is the source of
//! truth) and is only logged in `shadow`. A verb that targets a lease with no
//! entity (unnamed, or created before the mode) is untouched.

use trogon_sandbox_contract::EntityRef;
use trogon_sandbox_contract::SandboxEntityId;

use super::*;
use crate::entity_shadow::{Intent, Recorded};

/// An intent this verb wrote, kept so a failed verb can take it back.
pub(super) struct WrittenIntent {
    tenant: TenantId,
    entity: SandboxEntityId,
    lease: LeaseId,
}

impl BrokerCore {
    /// Records `intent` for the entity bound to `lease`. `Ok(Some(_))` only when
    /// this call changed the record.
    pub(super) async fn entity_intend(
        &self,
        tenant: &TenantId,
        lease: &LeaseId,
        intent: Intent,
    ) -> Result<Option<WrittenIntent>, LeaseError> {
        let Some(shadow) = &self.entity else {
            return Ok(None);
        };
        let record = match self.active_leases.get(lease).await {
            Ok(Some(record)) if &record.tenant == tenant => record,
            // The verb itself answers for a missing or foreign lease.
            Ok(_) => return Ok(None),
            Err(error) => {
                warn!(error = %error, "entity intent could not read the lease");
                return Ok(None);
            }
        };
        let located = match (&record.entity_id, &record.name) {
            (Some(id), _) => Ok(Some(id.clone())),
            (None, Some(name)) => shadow
                .store()
                .get_by_name(tenant, name.as_str())
                .await
                .map(|found| found.map(|v| v.record.entity_id)),
            (None, None) => Ok(None),
        };
        let id = match located {
            Ok(Some(id)) => id,
            Ok(None) => return Ok(None),
            Err(error) => return Self::intent_failed(shadow.is_authoritative(), intent, error),
        };
        match shadow.record_intent(tenant, &id, Some(lease), intent).await {
            Ok(Recorded::Written) => Ok(Some(WrittenIntent {
                tenant: tenant.clone(),
                entity: id,
                lease: lease.clone(),
            })),
            Ok(Recorded::Already | Recorded::NotApplicable) => Ok(None),
            Err(error) => Self::intent_failed(shadow.is_authoritative(), intent, error),
        }
    }

    /// Records the delete intent on the entity `entity` names, if there is one.
    pub(super) async fn entity_intend_delete(&self, tenant: &TenantId, entity: &EntityRef) -> Result<(), LeaseError> {
        let Some(shadow) = &self.entity else {
            return Ok(());
        };
        let found = match entity {
            EntityRef::Id(id) => match shadow.store().get(tenant, id).await {
                Err(crate::entity_store::EntityStoreError::NotFound) => Ok(None),
                other => other,
            },
            EntityRef::Name(name) => shadow.store().get_by_name(tenant, name).await,
        };
        let id = match found {
            Ok(Some(found)) => found.record.entity_id,
            Ok(None) => return Ok(()),
            Err(error) => {
                return Self::intent_failed(shadow.is_authoritative(), Intent::Delete, error).map(|_| ());
            }
        };
        match shadow.record_intent(tenant, &id, None, Intent::Delete).await {
            Ok(_) => Ok(()),
            Err(error) => Self::intent_failed(shadow.is_authoritative(), Intent::Delete, error).map(|_| ()),
        }
    }

    /// Takes back an intent whose verb did not take effect, so the reconciler
    /// does not keep retrying something the user was told failed. Best effort:
    /// if the write fails the entity keeps the intent and `on` mode retries it.
    pub(super) async fn entity_take_back(&self, written: Option<WrittenIntent>, opposite: Intent) {
        let (Some(written), Some(shadow)) = (written, &self.entity) else {
            return;
        };
        if let Err(error) = shadow
            .record_intent(&written.tenant, &written.entity, Some(&written.lease), opposite)
            .await
        {
            warn!(error = %error, "entity intent could not be taken back");
        }
    }

    fn intent_failed(
        authoritative: bool,
        intent: Intent,
        error: crate::entity_store::EntityStoreError,
    ) -> Result<Option<WrittenIntent>, LeaseError> {
        if authoritative {
            warn!(error = %error, ?intent, "sandbox entity intent write failed; denying the request");
            Err(LeaseError::EntityUnavailable {
                reason: format!("record {intent:?}: {error}"),
            })
        } else {
            warn!(error = %error, ?intent, "sandbox entity intent write failed; request unaffected");
            Ok(None)
        }
    }
}

// The verbs that write intent. They live here, next to the writers, and run
// the unchanged lease logic in `core.rs` once the intent is on the record.
impl BrokerCore {
    /// Stops a lease's current session while keeping it (and anything tagged
    /// under its `name`) resumable later: the public, tenant-facing
    /// counterpart to [`Self::handle_cancel`]. See [`StopLeaseRequest`]'s doc
    /// comment for the stop/cancel/delete distinction.
    pub async fn handle_stop_lease(&self, req: StopLeaseRequest) -> StopLeaseResponse {
        let record = match self.active_leases.get(&req.lease_id).await {
            Ok(Some(r)) => r,
            Ok(None) => {
                warn!(lease_id = %req.lease_id, "stop lease denied: lease not found");
                return StopLeaseResponse::Denied(LeaseError::LeaseNotFound { lease_id: req.lease_id });
            }
            Err(e) => {
                warn!(error = %e, "stop lease denied: active lease lookup failed");
                return StopLeaseResponse::Denied(LeaseError::Backend { message: e.to_string() });
            }
        };

        if record.tenant != req.tenant {
            warn!(lease_id = %req.lease_id, "stop lease denied: tenant mismatch");
            return StopLeaseResponse::Denied(LeaseError::LeaseNotFound { lease_id: req.lease_id });
        }

        if let Err(e) = self
            .entity_intend(&req.tenant, &req.lease_id, crate::entity_shadow::Intent::Stop)
            .await
        {
            return StopLeaseResponse::Denied(e);
        }

        if let Err(e) = self.cancel_lease(req.lease_id.clone(), CancelIntent::Release).await {
            warn!(error = %e, lease_id = %req.lease_id, "stop lease denied: cancel failed");
            return StopLeaseResponse::Denied(LeaseError::Backend { message: e.to_string() });
        }

        info!(lease_id = %req.lease_id, "lease stopped");
        StopLeaseResponse::Stopped
    }

    /// Freezes a running lease, recording the pause on the entity first.
    pub async fn handle_pause_lease(&self, req: PauseLeaseRequest) -> PauseLeaseResponse {
        let written = match self
            .entity_intend(&req.tenant, &req.lease_id, crate::entity_shadow::Intent::Pause)
            .await
        {
            Ok(written) => written,
            Err(error) => return PauseLeaseResponse::Denied(error),
        };
        let response = self.drive_pause_lease(req).await;
        if !matches!(response, PauseLeaseResponse::Paused { .. }) {
            self.entity_take_back(written, crate::entity_shadow::Intent::Resume)
                .await;
        }
        response
    }

    /// Thaws a paused lease, recording the running state on the entity first.
    pub async fn handle_resume_lease(&self, req: ResumeLeaseRequest) -> ResumeLeaseResponse {
        let written = match self
            .entity_intend(&req.tenant, &req.lease_id, crate::entity_shadow::Intent::Resume)
            .await
        {
            Ok(written) => written,
            Err(error) => return ResumeLeaseResponse::Denied(error),
        };
        let response = self.drive_resume_lease(req).await;
        if !matches!(response, ResumeLeaseResponse::Resumed { .. }) {
            self.entity_take_back(written, crate::entity_shadow::Intent::Pause)
                .await;
        }
        response
    }
}

//! Entity hooks on the lease paths. Every hook is a no-op unless an
//! [`EntityShadow`] was installed.
//!
//! In `shadow` mode no hook can fail or alter a response: the legacy stores
//! stay authoritative and an entity error is only logged. In `on` mode the
//! entity is the source of truth, so a failure that happens before a lease
//! exists (binding it, adopting a running one, resolving a name, deleting)
//! fails the request with [`LeaseError::EntityUnavailable`]. Hooks that run
//! after the lease is real (`lease_ready`, `run_ended`) stay best effort in
//! both modes: failing there would strand a live lease, and the repair sweep
//! reconciles the entity.

use std::time::Instant;

use trogon_sandbox_contract::Digest;
use trogon_sandbox_contract::SandboxEntityId;

use super::*;
use crate::entity_shadow::{EntityShadow, RepairReport};
use crate::entity_store::EntityStoreError;
use trogon_sandbox_contract::LeaseName;

impl BrokerCore {
    /// Installs the entity mirror. Never call it for mode `off`.
    pub fn with_entity_shadow(mut self, shadow: Arc<EntityShadow>) -> Self {
        self.entity = Some(shadow);
        self
    }

    /// Only named persistent requests belong to an entity.
    fn entity_for<'a>(&'a self, req: &'a LeaseRequest) -> Option<(&'a Arc<EntityShadow>, &'a LeaseName)> {
        let shadow = self.entity.as_ref()?;
        let name = req.name.as_ref()?;
        req.persistent.is_persistent().then_some((shadow, name))
    }

    /// Settles an entity write: an error fails the request in `on` mode and is
    /// logged in `shadow` mode.
    fn entity_outcome<T>(
        shadow: &EntityShadow,
        op: &str,
        result: Result<T, EntityStoreError>,
    ) -> Result<Option<T>, LeaseError> {
        match result {
            Ok(value) => Ok(Some(value)),
            Err(error) if shadow.is_authoritative() => {
                warn!(error = %error, op, "sandbox entity write failed; denying the request");
                Err(LeaseError::EntityUnavailable {
                    reason: format!("{op}: {error}"),
                })
            }
            Err(error) => {
                warn!(error = %error, op, "sandbox entity shadow write failed; lease response unaffected");
                Ok(None)
            }
        }
    }

    /// A named request is returning a lease that was already running.
    pub(super) async fn entity_stamp_existing(
        &self,
        req: &LeaseRequest,
        granted: &mut LeaseGranted,
    ) -> Result<(), LeaseError> {
        let Some((shadow, name)) = self.entity_for(req) else {
            return Ok(());
        };
        match self.active_leases.get(&granted.lease_id).await {
            Ok(Some(record)) if record.tenant == req.tenant => {
                let adopted = shadow.adopt_running(&req.tenant, name.as_str(), &record).await;
                granted.entity_id = Self::entity_outcome(shadow, "adopt_running", adopted)?;
            }
            Ok(_) => {}
            Err(error) => {
                warn!(error = %error, "entity could not read the running lease");
                if shadow.is_authoritative() {
                    return Err(LeaseError::EntityUnavailable {
                        reason: format!("read running lease: {error}"),
                    });
                }
            }
        }
        Ok(())
    }

    /// The name claim is won and `lease_id` is about to be provisioned.
    pub(super) async fn entity_begin_start(
        &self,
        req: &LeaseRequest,
        lease_id: &LeaseId,
        from_digest: Option<&Digest>,
    ) -> Result<Option<SandboxEntityId>, LeaseError> {
        let Some((shadow, name)) = self.entity_for(req) else {
            return Ok(None);
        };
        let started = shadow
            .begin_start(&req.tenant, name.as_str(), lease_id, &req.spec, &req.tags, from_digest)
            .await;
        Self::entity_outcome(shadow, "begin_start", started)
    }

    /// The lease a named request would reconnect to. In `on` mode the entity
    /// answers; a name it has never seen falls back to the legacy index, which
    /// is how a sandbox that predates the mode is found. Elsewhere only the
    /// legacy index is read.
    pub(super) async fn resolve_named_lease(
        &self,
        tenant: &TenantId,
        name: &str,
    ) -> Result<Option<LeaseId>, LeaseError> {
        if let Some(shadow) = self.entity.as_ref().filter(|s| s.is_authoritative()) {
            match shadow.current_lease_by_name(tenant, name).await {
                Ok(Some(current)) => return Ok(current),
                Ok(None) => {}
                Err(error) => {
                    warn!(error = %error, "sandbox entity name lookup failed");
                    return Err(LeaseError::EntityUnavailable {
                        reason: format!("resolve name: {error}"),
                    });
                }
            }
        }
        self.named_leases
            .get(tenant, name)
            .await
            .map_err(|e| LeaseError::Backend { message: e.to_string() })
    }

    pub(super) async fn entity_finish_start(
        &self,
        tenant: &TenantId,
        name: Option<&str>,
        claim: Option<&LeaseId>,
        response: &LeaseResponse,
    ) {
        let Some(shadow) = &self.entity else { return };
        match response {
            LeaseResponse::Granted(granted) if granted.created.is_created() => {
                if let Some(id) = &granted.entity_id {
                    shadow.lease_ready(tenant, id, &granted.lease_id).await;
                }
            }
            LeaseResponse::Denied(_) => {
                if let (Some(name), Some(lease)) = (name, claim) {
                    shadow.start_failed(tenant, name, lease).await;
                }
            }
            LeaseResponse::Granted(_) => {}
        }
    }

    /// `record`'s lease is being released. Call before the release event is
    /// published, so the persistence tag still holds the previous digest.
    pub(super) async fn entity_run_ended(&self, record: &LeaseRecord, reason: &str) {
        if let Some(shadow) = &self.entity {
            shadow.run_ended(record, Arc::clone(&self.tag_store), reason).await;
        }
    }

    /// A snapshot was published. Records it as the entity's head when the
    /// builder says it re-pointed the entity's persistence tag.
    pub async fn entity_snapshot_published(&self, event: &trogon_sandbox_contract::SnapshotPublished) {
        if let Some(shadow) = &self.entity {
            shadow.snapshot_published(event).await;
        }
    }

    /// The named sandbox was deleted. `Ok(true)` when an entity was removed.
    pub(super) async fn entity_deleted(&self, tenant: &TenantId, name: &str) -> Result<bool, LeaseError> {
        let Some(shadow) = &self.entity else {
            return Ok(false);
        };
        let removed = shadow.delete_named(tenant, name).await;
        Ok(Self::entity_outcome(shadow, "delete", removed)?.unwrap_or(false))
    }

    /// One entity repair pass; a no-op unless the entity mirror is installed.
    pub async fn sweep_entities(&self) -> RepairReport {
        match &self.entity {
            Some(shadow) => shadow.repair(self.active_leases.as_ref(), Instant::now()).await,
            None => RepairReport::default(),
        }
    }
}

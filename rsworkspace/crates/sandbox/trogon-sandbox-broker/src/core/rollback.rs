//! Rollback: rebuild a running lease's sandbox from a chosen snapshot while
//! keeping its lease id, tenant and deadline.
//!
//! The lease record does not change; only the sandbox behind it does. The
//! worker holding the lease replies once the new sandbox is up, and treats a
//! repeat as success, so the operation driver can re-drive a rollback whose
//! reply was lost. A failed rollback leaves the lease `Running`; the worker
//! says in its denial whether the old sandbox is still intact.

use trogon_sandbox_contract::{
    RollbackLeaseRequest, RollbackLeaseResponse, SandboxRollbackRequest, SandboxRollbackResponse,
};

use super::*;

impl BrokerCore {
    /// Resets a lease to a snapshot, keeping its id. Idempotent under an
    /// operation id.
    pub async fn handle_rollback_lease(&self, req: RollbackLeaseRequest) -> RollbackLeaseResponse {
        let Some(operations) = self.operations.clone() else {
            return self.rollback_lease_inline(req).await;
        };
        match self
            .run_operation(
                &operations,
                &req.tenant,
                &req.lease_id,
                req.operation_id,
                OperationRequest::Rollback { snapshot: req.snapshot },
            )
            .await
        {
            Ok((operation_id, OperationResult::RolledBack)) => RollbackLeaseResponse::RolledBack {
                operation_id: Some(operation_id),
            },
            Ok((_, OperationResult::Unsupported)) => RollbackLeaseResponse::Unsupported,
            Ok((_, other)) => RollbackLeaseResponse::Denied(unexpected_result(&other)),
            Err(error) => RollbackLeaseResponse::Denied(error),
        }
    }

    /// Rolls back without recording an operation. The operation driver and the
    /// no-store fallback run this.
    pub(crate) async fn rollback_lease_inline(&self, req: RollbackLeaseRequest) -> RollbackLeaseResponse {
        let record = match self.owned_lease(&req.tenant, &req.lease_id).await {
            Ok(r) => r,
            Err(e) => return RollbackLeaseResponse::Denied(e),
        };
        if record.status != LeaseStatus::Running {
            return RollbackLeaseResponse::Denied(LeaseError::LeaseNotRunning { lease_id: req.lease_id });
        }
        // Only the tenant's own snapshots: the repository is derived from the
        // tenant, so a reference into another tenant's repository is refused.
        if req.snapshot.registry != self.config.snapshot_registry
            || req.snapshot.repository != snapshot_repository_for_tenant(&req.tenant)
        {
            warn!(lease_id = %req.lease_id, "rollback denied: snapshot is not the tenant's own");
            return RollbackLeaseResponse::Denied(LeaseError::InvalidSpec {
                reason: "rollback snapshot is not in the tenant's snapshot repository".to_string(),
            });
        }
        // An unknown provider is treated as able, as pause does; the worker
        // answers Unsupported if it is not.
        if let Some(profile) = self.worker_registry.profile_for_provider(record.provider).await
            && !profile.snapshot.supports_in_place_rollback.is_supported()
        {
            return RollbackLeaseResponse::Unsupported;
        }

        let body = match serde_json::to_vec(&SandboxRollbackRequest {
            lease_id: req.lease_id.clone(),
            snapshot: req.snapshot,
        }) {
            Ok(body) => Bytes::from(body),
            Err(e) => return RollbackLeaseResponse::Denied(backend(e)),
        };
        let subject = subjects::provider_rollback(record.provider, &req.lease_id);
        let reply = match self.bus.request(&subject, body).await {
            Ok(reply) => reply,
            Err(e) => {
                warn!(error = %e, lease_id = %req.lease_id, "rollback dispatch failed");
                return RollbackLeaseResponse::Denied(backend(e));
            }
        };
        match serde_json::from_slice::<SandboxRollbackResponse>(&reply) {
            Ok(SandboxRollbackResponse::Done) => {
                info!(lease_id = %req.lease_id, "lease rolled back");
                RollbackLeaseResponse::RolledBack { operation_id: None }
            }
            Ok(SandboxRollbackResponse::Unsupported) => RollbackLeaseResponse::Unsupported,
            Ok(SandboxRollbackResponse::Denied(e)) => RollbackLeaseResponse::Denied(e),
            Err(e) => RollbackLeaseResponse::Denied(backend(format!("rollback reply decode failed: {e}"))),
        }
    }
}

use serde_json::Value;
use trogon_sandbox_contract::{
    CancelOwnedLeaseRequest, LeaseAttemptIdentity, LeaseId, LeaseRequest, ProviderKind, ProviderLease, SandboxId,
    SandboxImage,
};
use trogon_sandbox_protocol::{
    optional_bool, options_for, reject_unknown_keys, reject_unsupported_privileged, subjects,
};

use super::constants::{AUTO_PAUSE_OPTION, NATIVE_OPTION_KEYS};
use super::{ActiveLease, E2bProviderError, E2bWorkerProvider, LeaseAccess, LeaseLifetime, PauseOwnership};
use crate::api::{ApiError, AutoPause, MAX_TIMEOUT_SECS, NewSandbox, TrafficPrivacy};
use crate::assignments::validate_sandbox_id;
use crate::envd::EnvdTarget;
use crate::network::translate_network_policy;
use crate::profile::PROFILE;
use crate::snapshot_custody::CustodyPhase;
use crate::snapshots::validate_reference;

/// Validates `native.e2b` against the allowlist. Refusing here, before any sandbox exists, keeps a typo from costing
/// a create call.
fn native_params(native: Option<&Value>) -> Result<Option<bool>, E2bProviderError> {
    let options = options_for(ProviderKind::E2b, native)?;
    reject_unknown_keys(&options, NATIVE_OPTION_KEYS)?;
    Ok(optional_bool(&options, AUTO_PAUSE_OPTION)?)
}

impl E2bWorkerProvider {
    pub(super) async fn allocate_lease(
        &self,
        req: &LeaseRequest,
        lease_id: &LeaseId,
        assignment: Option<&LeaseAttemptIdentity>,
    ) -> Result<ProviderLease, E2bProviderError> {
        let gate = self.lease_gate(lease_id);
        let _held = gate.lock().await;
        if self.active.lock().await.contains_key(lease_id.as_str()) {
            return Err(E2bProviderError::AlreadyHeld);
        }
        reject_unsupported_privileged(&req.spec, &PROFILE)?;
        if !req.spec.mounts().is_empty() {
            return Err(E2bProviderError::MountsUnsupported);
        }
        let auto_pause = native_params(req.native.as_ref())?;
        let network = translate_network_policy(req.spec.network_policy())?;
        let snapshot_gate = match req.spec.image() {
            SandboxImage::NativeSnapshot { reference } => Some(self.snapshot_gate(&req.tenant, reference)),
            _ => None,
        };
        let _snapshot_guard = match &snapshot_gate {
            Some(gate) => Some(gate.lock().await),
            None => None,
        };
        let manifest = if let SandboxImage::NativeSnapshot { reference } = req.spec.image() {
            validate_reference(reference)?;
            if let Some(stored) = self.custody.load(&req.tenant, reference).await?
                && matches!(stored.record.phase, CustodyPhase::Cleanup | CustodyPhase::Deleted)
            {
                return Err(E2bProviderError::SnapshotDeletionStarted);
            }
            Some(
                self.snapshots
                    .load(&req.tenant, reference)
                    .await?
                    .ok_or(E2bProviderError::SnapshotNotOwned)?,
            )
        } else {
            None
        };
        let template_id = match &manifest {
            Some(manifest) => manifest.restore_binding()?.restore_selector(),
            None => self.template_for(req.spec.image())?,
        };
        let new = NewSandbox {
            template_id,
            timeout: req.ttl_seconds.clamp(1, MAX_TIMEOUT_SECS),
            auto_pause: auto_pause.map(AutoPause::from),
            allow_internet_access: network.allow_internet_access,
            network: network.network,
            metadata: req
                .tags
                .iter()
                .map(|(key, value)| (key.to_owned(), value.to_owned()))
                .collect(),
            env_vars: req.spec.env().clone(),
        };
        let info = self.api.create_sandbox(&new).await?;
        let created_id = match SandboxId::new(info.sandbox_id.clone()) {
            Ok(id) => id,
            Err(error) => {
                // Do not leak a sandbox the protocol cannot name.
                let _ = self.api.delete_sandbox(&info.sandbox_id).await;
                return Err(error.into());
            }
        };
        let traffic_privacy = if info.traffic_access_token.is_some() {
            TrafficPrivacy::Private
        } else {
            TrafficPrivacy::Public
        };
        if let Some(identity) = assignment {
            validate_sandbox_id(&info.sandbox_id)?;
            let receipt = ProviderLease {
                sandbox_id: created_id.clone(),
                evidence_subject: subjects::evidence_subject(lease_id),
                worker_id: Some(identity.target.worker_id.clone()),
                attempt_id: Some(identity.target.attempt_id.clone()),
                epoch: identity.target.epoch,
            };
            self.dispatch_ledger()
                .ok_or(E2bProviderError::LedgerMissing)?
                .record_backend_receipt(identity, receipt)
                .await?;
            self.assignments.bind_native(identity, &info.sandbox_id).await?;
        }
        let config = self.api.config();
        let domain = info.domain.clone().unwrap_or_else(|| config.domain.clone());
        let target =
            match EnvdTarget::for_sandbox(config, &info.sandbox_id, Some(&domain), info.envd_access_token.clone()) {
                Ok(target) => target,
                Err(error) => {
                    // Do not leak a sandbox the worker cannot address.
                    let _ = self.api.delete_sandbox(&info.sandbox_id).await;
                    return Err(error.into());
                }
            };
        let mut held_lease = ActiveLease {
            access: LeaseAccess::Ready,
            tenant: req.tenant.clone(),
            gate: gate.clone(),
            pause: PauseOwnership::Unowned,
            sandbox_id: info.sandbox_id.clone(),
            target,
            traffic_token: info.traffic_access_token.clone(),
            traffic_privacy,
            domain,
            exposed_ports: req.spec.exposed_ports().iter().map(|port| port.get()).collect(),
            lifetime: LeaseLifetime::Requested(req.ttl_seconds.max(1)),
            region: req.region.clone(),
        };
        if let Some(manifest) = &manifest
            && let Err(restore) = self
                .exec
                .restore_executions(&held_lease.view(), lease_id.as_str(), &manifest.executions)
                .await
        {
            return match self.api.delete_sandbox(&info.sandbox_id).await {
                Ok(_) | Err(ApiError::NotFound(_)) => Err(restore.into()),
                Err(cleanup) => {
                    held_lease.access = LeaseAccess::CleanupOnly;
                    self.active
                        .lock()
                        .await
                        .insert(lease_id.as_str().to_owned(), held_lease);
                    Err(E2bProviderError::RestoreCleanupFailed { restore, cleanup })
                }
            };
        }
        self.exec
            .record_start_baseline(lease_id.as_str(), &held_lease.target)
            .await;
        self.active
            .lock()
            .await
            .insert(lease_id.as_str().to_owned(), held_lease);
        Ok(ProviderLease {
            epoch: assignment.and_then(|identity| identity.target.epoch),
            sandbox_id: created_id,
            evidence_subject: subjects::evidence_subject(lease_id),
            worker_id: self.worker_id.get().cloned(),
            attempt_id: assignment.map(|identity| identity.target.attempt_id.clone()),
        })
    }

    pub(super) async fn cancel_lease(&self, lease_id: &LeaseId) -> Result<(), E2bProviderError> {
        let gate = self.lease_gate(lease_id);
        let _held = gate.lock().await;
        let request = {
            let mut active = self.active.lock().await;
            match active.get_mut(lease_id.as_str()) {
                Some(lease) => {
                    lease.access = LeaseAccess::CleanupOnly;
                    Some((lease.tenant.clone(), lease.sandbox_id.clone()))
                }
                None => None,
            }
        };
        let Some((tenant, sandbox_id)) = request else {
            return Ok(());
        };
        let request = CancelOwnedLeaseRequest {
            tenant,
            lease_id: lease_id.clone(),
            sandbox_id: SandboxId::new(sandbox_id)?,
        };
        if self.cleanup_receipts.is_configured() {
            let stored = self.cleanup_receipts.begin(&request).await?;
            if !stored.absent() {
                self.cancel_journaled_resource(&request, stored).await?;
            }
        } else {
            self.delete_sandbox_confirmed(request.sandbox_id.as_str()).await?;
        }
        self.forget_exact(lease_id, request.sandbox_id.as_str()).await;
        Ok(())
    }
}

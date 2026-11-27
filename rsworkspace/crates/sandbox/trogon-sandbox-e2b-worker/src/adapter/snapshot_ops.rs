use async_nats::jetstream;
use trogon_sandbox_contract::{
    LeaseId, NativeSnapshotId, NativeSnapshotReceipt, NativeSnapshotRef, ProviderKind, SnapshotExportError,
    SnapshotExportOutcome,
};

use super::constants::SNAPSHOT_MANIFEST_VERSION;
use super::{E2bProviderError, E2bWorkerProvider, LeaseAccess, LeaseLifetime};
use crate::api::ApiError;
use crate::snapshot_custody::CustodyPhase;
use crate::snapshots::{RecordOutcome, SnapshotManifest, validate_logical_name};
use crate::ulid::Ulid;

fn permanent(message: impl Into<String>) -> SnapshotExportOutcome {
    SnapshotExportOutcome::Err(SnapshotExportError::Permanent {
        message: message.into(),
    })
}

fn transient(message: impl Into<String>) -> SnapshotExportOutcome {
    SnapshotExportOutcome::Err(SnapshotExportError::Transient {
        message: message.into(),
    })
}

impl E2bWorkerProvider {
    pub(crate) fn configure_snapshots(&self, context: jetstream::Context) -> Result<(), E2bProviderError> {
        self.snapshots.configure(context.clone())?;
        self.custody.configure(context.clone())?;
        self.cleanup_receipts.configure(context)?;
        Ok(())
    }

    pub(super) async fn resume_lease(
        &self,
        lease_id: &LeaseId,
        sandbox_id: &str,
        lifetime: LeaseLifetime,
    ) -> Result<(), ApiError> {
        let info = self
            .api
            .connect_sandbox(sandbox_id, lifetime.remaining_seconds()?)
            .await?;
        if info.sandbox_id != sandbox_id {
            return Err(ApiError::Transport(
                "resume returned a different sandbox identity".into(),
            ));
        }
        let mut active = self.active.lock().await;
        let lease = active
            .get_mut(lease_id.as_str())
            .ok_or_else(|| ApiError::NotFound("lease no longer held".into()))?;
        lease.target.replace_token(info.envd_access_token);
        if info.traffic_access_token.is_some() {
            lease.traffic_token = info.traffic_access_token;
        }
        Ok(())
    }

    fn new_snapshot_reference(id: String) -> Result<NativeSnapshotRef, E2bProviderError> {
        Ok(NativeSnapshotRef {
            provider: ProviderKind::E2b,
            id: NativeSnapshotId::new(id)?,
            expires_at: None,
        })
    }

    pub async fn snapshot_lease(&self, lease_id: &LeaseId, name: &str) -> SnapshotExportOutcome {
        self.snapshot_lease_as(lease_id, name, Ulid::generate().to_string())
            .await
    }

    /// Captures the lease under the snapshot identity `id`.
    pub(super) async fn snapshot_lease_as(&self, lease_id: &LeaseId, name: &str, id: String) -> SnapshotExportOutcome {
        if let Err(error) = validate_logical_name(name) {
            return permanent(error.to_string());
        }
        let gate = match self.active.lock().await.get(lease_id.as_str()) {
            Some(lease) if lease.access == LeaseAccess::Ready => lease.gate.clone(),
            _ => return permanent("lease is not held by this worker"),
        };
        let _lease_gate = gate.lock().await;
        let (sandbox_id, tenant, lifetime) = match self.active.lock().await.get(lease_id.as_str()) {
            Some(lease) if lease.access == LeaseAccess::Ready => {
                if lease.pause.is_authority() {
                    return transient("the sandbox is held in an authority pause");
                }
                (lease.sandbox_id.clone(), lease.tenant.clone(), lease.lifetime)
            }
            _ => return permanent("lease was released before capture"),
        };
        let _executions = self.exec.freeze_executions(lease_id.as_str()).await;
        let captured = async {
            let found = self.exec.capture_executions(lease_id.as_str()).await;
            let executions = found.map_err(|e| e.to_string())?;
            let reference = Self::new_snapshot_reference(id).map_err(|e| e.to_string())?;
            Ok::<_, String>((executions, reference))
        }
        .await;
        let (executions, reference) = match captured {
            Ok(captured) => captured,
            Err(message) => return permanent(message),
        };
        let prepared = match self.begin_snapshot_capture(&tenant, &reference).await {
            Ok(prepared) => prepared,
            Err(error) => {
                return transient(format!(
                    "snapshot metadata custody is unavailable before capture: {error}"
                ));
            }
        };
        let created = match self.api.create_snapshot(&sandbox_id).await {
            Ok(created) => created,
            Err(error) => {
                let message = match self.resume_lease(lease_id, &sandbox_id, lifetime).await {
                    Ok(()) => error.to_string(),
                    Err(resume) => format!("{error}; source resume failed: {resume}"),
                };
                let is_permanent = matches!(
                    error,
                    ApiError::Unauthorized(_) | ApiError::NotFound(_) | ApiError::Rejected { .. }
                );
                return if is_permanent {
                    permanent(message)
                } else {
                    transient(message)
                };
            }
        };
        let mut owned = prepared.record;
        owned.template_id = Some(created.template_id.clone());
        owned.phase = CustodyPhase::Owned;
        if let Err(error) = self.custody.save(owned, prepared.revision).await {
            let resumed = self.resume_lease(lease_id, &sandbox_id, lifetime).await;
            return self
                .snapshot_cleanup(
                    &tenant,
                    &reference,
                    &created.template_id,
                    format!("captured snapshot ownership could not be persisted: {error}; source resume: {resumed:?}"),
                )
                .await;
        }
        if let Err(error) = self.resume_lease(lease_id, &sandbox_id, lifetime).await {
            return self
                .snapshot_cleanup(
                    &tenant,
                    &reference,
                    &created.template_id,
                    format!("snapshot captured but source resume failed: {error}"),
                )
                .await;
        }
        let binding = match self.api.resolve_snapshot(&created).await {
            Ok(binding) => binding,
            Err(error) => {
                return self
                    .snapshot_cleanup(
                        &tenant,
                        &reference,
                        &created.template_id,
                        format!("captured snapshot immutable binding could not be resolved: {error}"),
                    )
                    .await;
            }
        };
        let manifest = SnapshotManifest {
            version: SNAPSHOT_MANIFEST_VERSION,
            tenant: tenant.clone(),
            reference: reference.clone(),
            executions,
            binding: Some(binding),
            logical_name: Some(name.to_owned()),
        };
        match self.snapshots.record(&manifest).await {
            Ok(RecordOutcome::Created) => match self.keep_snapshot(&tenant, &reference).await {
                Ok(()) => SnapshotExportOutcome::Native(NativeSnapshotReceipt { reference, bytes: None }),
                Err(error) => transient(format!(
                    "snapshot metadata is retained but capture custody finalization failed: {error}"
                )),
            },
            Ok(RecordOutcome::AlreadyExists) => {
                self.snapshot_cleanup(
                    &tenant,
                    &reference,
                    &created.template_id,
                    "snapshot metadata identity already exists; existing snapshot was retained".into(),
                )
                .await
            }
            Err(error) => {
                self.snapshot_cleanup(
                    &tenant,
                    &reference,
                    &created.template_id,
                    format!("snapshot metadata could not be persisted: {error}"),
                )
                .await
            }
        }
    }
}

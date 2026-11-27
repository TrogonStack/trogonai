use std::time::Duration;

use trogon_sandbox_contract::{
    LeaseId, NativeSnapshotId, NativeSnapshotReceipt, NativeSnapshotRef, ProviderKind, SnapshotExportError,
    SnapshotExportOutcome,
};

use crate::check::ensure;
use crate::constants::{POLL, SECONDS_PER_DAY};
use crate::create_requests::CreateVolumeBody;
use crate::{FlyApiError, FlyProvider, FlyProviderError, FlySnapshotRef, FlyVolumeId, FlyWorkspaceVolume};

impl FlyProvider {
    fn volume_prefix(&self) -> String {
        format!("sbxw_{}_", &self.owner[..8])
    }

    /// Creates the lease's workspace volume, fresh or restored from a snapshot, and waits until Fly reports it
    /// usable.
    pub(crate) async fn create_workspace_volume(
        &self,
        settings: &FlyWorkspaceVolume,
        snapshot: Option<&str>,
    ) -> Result<FlyVolumeId, FlyProviderError> {
        let name = format!(
            "{}{}",
            self.volume_prefix(),
            &uuid::Uuid::new_v4().simple().to_string()[..12]
        );
        let body = CreateVolumeBody {
            name: name.as_str(),
            region: self.config.region.as_str(),
            size_gb: settings.size_gb().get(),
            encrypted: true,
            snapshot_id: snapshot,
        };
        let body = serde_json::to_value(&body).map_err(FlyApiError::Decode)?;
        let id = match self.api.create_volume(&body).await {
            Ok(created) => created.id,
            Err(error) => {
                // A timed-out create may have succeeded; reclaim only this name.
                for volume in self.api.list_volumes().await? {
                    if volume.name == name {
                        self.delete_workspace_volume(&volume.id).await?;
                    }
                }
                return Err(error.into());
            }
        };
        let ready = tokio::time::timeout(self.config.boot_timeout, async {
            loop {
                match self.api.volume(&id).await?.state.as_str() {
                    "created" => return Ok(()),
                    "destroyed" | "failed" => return Err(FlyProviderError::Backend("workspace volume failed")),
                    _ => tokio::time::sleep(POLL).await,
                }
            }
        })
        .await
        .map_err(|_| FlyProviderError::TimedOut("workspace volume readiness"))
        .and_then(|result| result);
        if let Err(error) = ready {
            self.discard_workspace_volume(Some(&id)).await;
            return Err(error);
        }
        Ok(id)
    }

    /// Deletes a volume once its Machine is gone. Fly rejects the delete while the volume is still attached,
    /// which can outlast the Machine destroy briefly, so a failure is retried before it is reported.
    pub(crate) async fn delete_workspace_volume(&self, id: &FlyVolumeId) -> Result<(), FlyProviderError> {
        let mut attempt = 0;
        loop {
            match self.api.delete_volume(id).await {
                Ok(()) => return Ok(()),
                Err(error) if attempt >= 4 => return Err(error.into()),
                Err(_) => {
                    attempt += 1;
                    tokio::time::sleep(Duration::from_millis(250 * attempt)).await;
                }
            }
        }
    }

    /// Best-effort cleanup on a failed lease; a volume that survives is swept by name at the next `recover`.
    pub(crate) async fn discard_workspace_volume(&self, id: Option<&FlyVolumeId>) {
        if let Some(id) = id
            && let Err(error) = self.delete_workspace_volume(id).await
        {
            tracing::warn!(%error, volume = id.as_str(), "workspace volume was not deleted");
        }
    }

    pub(crate) async fn sweep_workspace_volumes(&self) -> Result<(), FlyProviderError> {
        let prefix = self.volume_prefix();
        for volume in self.api.list_volumes().await? {
            let gone = matches!(volume.state.as_str(), "destroyed" | "pending_destroy");
            if volume.name.starts_with(&prefix) && !gone {
                self.delete_workspace_volume(&volume.id).await?;
            }
        }
        Ok(())
    }

    /// Takes a native snapshot of the lease's workspace volume. `None` means this worker does not hold the lease.
    pub async fn native_snapshot(&self, lease: &LeaseId) -> Option<SnapshotExportOutcome> {
        let record = self.record(lease).await.ok()?;
        let outcome = match (&self.config.workspace_volume, &record.workspace_volume) {
            (Some(settings), Some(volume)) => match self.snapshot_volume(settings, &record.machine, volume).await {
                Ok(receipt) => SnapshotExportOutcome::Native(receipt),
                Err(error) => SnapshotExportOutcome::Err(SnapshotExportError::Transient {
                    message: format!("Fly volume snapshot: {error}"),
                }),
            },
            _ => SnapshotExportOutcome::Err(SnapshotExportError::Unsupported {
                message: "Fly takes native snapshots only with a workspace volume".into(),
            }),
        };
        Some(outcome)
    }

    async fn snapshot_volume(
        &self,
        settings: &FlyWorkspaceVolume,
        machine: &crate::MachineId,
        volume: &FlyVolumeId,
    ) -> Result<NativeSnapshotReceipt, FlyProviderError> {
        // Flush dirty pages so the volume snapshot holds what the guest wrote.
        let flushed = self
            .api
            .exec(machine, vec!["/bin/sync".into()], Duration::from_secs(30))
            .await?;
        ensure(
            flushed.exit_code == 0,
            FlyProviderError::Backend("workspace flush failed"),
        )?;
        let id = self.api.create_snapshot(volume).await?;
        let entry = tokio::time::timeout(settings.snapshot_timeout(), async {
            loop {
                let found = self
                    .api
                    .list_snapshots(volume)
                    .await?
                    .into_iter()
                    .find(|entry| entry.id == id);
                if let Some(entry) = found {
                    match entry.status.as_deref() {
                        None | Some("created" | "complete" | "completed") => return Ok(entry),
                        Some("failed" | "error") => return Err(FlyProviderError::SnapshotFailed(id.clone())),
                        Some(_) => {}
                    }
                }
                tokio::time::sleep(settings.snapshot_poll_interval()).await;
            }
        })
        .await
        .map_err(|_| FlyProviderError::TimedOut("volume snapshot"))??;
        let reference = FlySnapshotRef::new(self.config.region.clone(), &id, &self.config.image)?;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .ok()
            .map(|elapsed| elapsed.as_secs());
        Ok(NativeSnapshotReceipt {
            reference: NativeSnapshotRef {
                provider: ProviderKind::Fly,
                id: NativeSnapshotId::new(reference.encode()).map_err(|_| FlyProviderError::Backend("snapshot id"))?,
                expires_at: entry
                    .retention_days
                    .zip(now)
                    .map(|(days, now)| now + days * SECONDS_PER_DAY),
            },
            bytes: entry.size,
        })
    }
}

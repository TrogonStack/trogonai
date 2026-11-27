use std::collections::BTreeMap;

use async_trait::async_trait;
use tracing::{info, warn};
use trogon_sandbox_contract::{ArtifactType, LeaseId, OciArtifactRef, SnapshotReceipt};
use trogon_sandbox_oci::Layer;

use crate::{ExportError, KubeVirtOps, SnapshotBackend, SnapshotService, SnapshotTiming};

#[async_trait]
impl SnapshotBackend for SnapshotService {
    async fn export(
        &self,
        ops: &KubeVirtOps,
        lease_id: &LeaseId,
        vm_name: &str,
        labels: BTreeMap<String, String>,
        target: &OciArtifactRef,
    ) -> Result<SnapshotReceipt, Box<dyn std::error::Error + Send + Sync>> {
        Ok(self
            .export_with_online_fallback(ops, lease_id, vm_name, labels, target)
            .await?)
    }

    async fn checkpoint(
        &self,
        ops: &KubeVirtOps,
        lease_id: &LeaseId,
        vm_name: &str,
        labels: BTreeMap<String, String>,
        target: &OciArtifactRef,
    ) -> Result<SnapshotReceipt, Box<dyn std::error::Error + Send + Sync>> {
        Ok(self
            .export_with_timing(ops, lease_id, vm_name, labels, target, SnapshotTiming::Online)
            .await?)
    }
}

impl SnapshotService {
    /// Exports while the VM keeps running, and falls back to stopping it only when the online attempt failed.
    async fn export_with_online_fallback(
        &self,
        ops: &KubeVirtOps,
        lease_id: &LeaseId,
        vm_name: &str,
        labels: BTreeMap<String, String>,
        target: &OciArtifactRef,
    ) -> Result<SnapshotReceipt, ExportError> {
        let online = self
            .export_with_timing(ops, lease_id, vm_name, labels.clone(), target, SnapshotTiming::Online)
            .await;
        let Err(error) = online else {
            return online;
        };
        warn!(%error, %vm_name, "the online snapshot failed; falling back to an offline export");
        ops.delete_vm_snapshot(&ops.snapshot_name(lease_id.as_str())).await.ok();
        self.export_with_timing(ops, lease_id, vm_name, labels, target, SnapshotTiming::Offline)
            .await
    }

    async fn export_with_timing(
        &self,
        ops: &KubeVirtOps,
        lease_id: &LeaseId,
        vm_name: &str,
        labels: BTreeMap<String, String>,
        target: &OciArtifactRef,
        timing: SnapshotTiming,
    ) -> Result<SnapshotReceipt, ExportError> {
        let was_running = ops.vmi_is_running(vm_name).await?;
        let stop = timing == SnapshotTiming::Offline && was_running;
        if timing == SnapshotTiming::Online && !was_running {
            return Err(ExportError::NotRunning {
                vm_name: vm_name.to_owned(),
            });
        }
        if stop {
            info!(%vm_name, "stopping the VM for an offline snapshot");
            ops.set_virtual_machine_running(vm_name, false).await?;
            ops.wait_vmi_stopped(vm_name).await?;
        }
        let exported = self
            .push_snapshot_disk(ops, &ops.snapshot_name(lease_id.as_str()), vm_name, labels, target)
            .await;
        if stop {
            info!(%vm_name, "restarting the VM after an offline snapshot");
            ops.set_virtual_machine_running(vm_name, true).await?;
            ops.wait_vmi_ready(vm_name).await?;
        }
        exported
    }

    async fn push_snapshot_disk(
        &self,
        ops: &KubeVirtOps,
        snapshot_name: &str,
        vm_name: &str,
        labels: BTreeMap<String, String>,
        target: &OciArtifactRef,
    ) -> Result<SnapshotReceipt, ExportError> {
        ops.create_vm_snapshot(snapshot_name, vm_name, labels).await?;
        ops.wait_vm_snapshot_ready(snapshot_name).await?;
        let pvc = ops.snapshot_pvc_name(snapshot_name).await?;
        info!(%snapshot_name, %pvc, %vm_name, "exporting the snapshot disk");
        let disk = ops.read_pvc_bytes(&pvc).await?;
        let bytes = disk.len() as u64;
        let layer = Layer {
            data: disk,
            media_type: ArtifactType::KubevirtDiskV1.media_type().to_owned(),
            annotations: Default::default(),
        };
        let stored = self.registry.push(target, layer).await?;
        Ok(SnapshotReceipt {
            digest: stored.digest,
            bytes,
            artifact_type: ArtifactType::KubevirtDiskV1,
        })
    }
}

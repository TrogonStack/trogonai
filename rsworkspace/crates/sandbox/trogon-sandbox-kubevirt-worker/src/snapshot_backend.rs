use std::collections::BTreeMap;

use async_trait::async_trait;
use trogon_sandbox_contract::{LeaseId, OciArtifactRef, SnapshotReceipt};

use crate::{KubeVirtOps, WarmDiskSource};

/// Restores and exports the disk snapshots of KubeVirt guests.
///
/// As a [`WarmDiskSource`] it also describes the root disk a snapshot lease or warm VM restores from.
#[async_trait]
pub trait SnapshotBackend: WarmDiskSource {
    /// Exports the disk of VM `vm_name` to `target`, online when the cluster can and otherwise by stopping the VM
    /// for the export and starting it again.
    async fn export(
        &self,
        ops: &KubeVirtOps,
        lease_id: &LeaseId,
        vm_name: &str,
        labels: BTreeMap<String, String>,
        target: &OciArtifactRef,
    ) -> Result<SnapshotReceipt, Box<dyn std::error::Error + Send + Sync>>;

    /// Exports the disk of the running VM `vm_name` to `target` without stopping it.
    async fn checkpoint(
        &self,
        ops: &KubeVirtOps,
        lease_id: &LeaseId,
        vm_name: &str,
        labels: BTreeMap<String, String>,
        target: &OciArtifactRef,
    ) -> Result<SnapshotReceipt, Box<dyn std::error::Error + Send + Sync>>;
}

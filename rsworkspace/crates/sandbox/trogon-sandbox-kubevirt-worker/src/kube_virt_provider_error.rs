use trogon_sandbox_contract::{ArtifactType, LeaseId, SandboxIdError};
use trogon_sandbox_protocol::AuthorityResourceIdError;

use crate::{CustodyError, KubeVirtOpsError, WarmDiskSourceError};

/// A [`crate::KubeVirtProvider`] operation failed or refused to act.
#[derive(Debug, thiserror::Error)]
pub enum KubeVirtProviderError {
    #[error("durable custody failed: {0}")]
    Custody(#[from] CustodyError),
    #[error("the Kubernetes operation failed: {0}")]
    Ops(#[from] KubeVirtOpsError),
    #[error("the restore disk could not be described: {0}")]
    DiskSource(#[from] WarmDiskSourceError),
    #[error("the snapshot backend failed: {0}")]
    Snapshot(Box<dyn std::error::Error + Send + Sync>),
    #[error("the sandbox identifier is invalid: {0}")]
    SandboxId(#[from] SandboxIdError),
    #[error("the authority resource identity is invalid: {0}")]
    ResourceId(#[from] AuthorityResourceIdError),
    #[error("KubeVirt lease ID is already held by this worker")]
    LeaseHeld,
    #[error("retained KubeVirt cleanup must settle before another allocation")]
    CleanupRetained,
    #[error("interrupted KubeVirt allocation custody must settle before another allocation")]
    AllocationInterrupted,
    #[error("exact durable custody is missing while resources remain held")]
    CustodyMissing,
    #[error("held DataVolume disappeared during recovery")]
    HeldDiskMissing,
    #[error("recovered resource belongs to another worker or lease")]
    ForeignResource,
    #[error("{count} KubeVirt allocations require manual durable ownership reconciliation")]
    Unresolved { count: usize },
    #[error("workspace {workspace} is not supported on kubevirt yet")]
    WorkspaceUnsupported { workspace: &'static str },
    #[error("kubevirt does not support spec.env: the VM boots from its image with no channel for it")]
    EnvUnsupported,
    #[error("SandboxImage::Snapshot restore requires volume store configuration")]
    VolumeUnconfigured,
    #[error("SnapshotTag must be resolved to Snapshot by the broker before lease")]
    SnapshotTagUnresolved,
    #[error("HyperlightModule is not valid on the kubevirt provider")]
    HyperlightUnsupported,
    #[error("NativeSnapshot is not valid on the kubevirt provider")]
    NativeSnapshotUnsupported,
    #[error("kubevirt snapshot export requires artifact_type KubevirtDiskV1, got {actual:?}")]
    ArtifactType { actual: ArtifactType },
    #[error("no active lease {lease_id}")]
    NoActiveLease { lease_id: LeaseId },
    #[error("checkpoint_running requires a running VMI for lease {lease_id}")]
    NotRunning { lease_id: LeaseId },
    #[error("VMI {vm_name} stopped during live checkpoint for lease {lease_id}")]
    StoppedDuringCheckpoint { vm_name: String, lease_id: LeaseId },
    #[error("the SSH session keys are already bound")]
    SshKeysBound,
}

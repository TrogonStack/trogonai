use trogon_sandbox_contract::DigestError;

use crate::CustodyError;

/// A KubeVirt cluster operation failed or refused to act on resources it could not prove it owns.
#[derive(Debug, thiserror::Error)]
pub enum KubeVirtOpsError {
    #[error("the Kubernetes API failed: {0}")]
    Kube(#[from] kube::Error),
    #[error("durable custody failed: {0}")]
    Custody(#[from] CustodyError),
    #[error("DataVolume name is missing")]
    MissingDataVolumeName,
    #[error("VM name is missing")]
    MissingVirtualMachineName,
    #[error("datavolume already exists: {0}")]
    DataVolumeExists(String),
    #[error("virtualmachine already exists: {0}")]
    VirtualMachineExists(String),
    #[error("DataVolume changed before import completed")]
    DataVolumeChanged,
    #[error("datavolume {name} failed: {message}")]
    DataVolumeFailed { name: String, message: String },
    #[error("timeout waiting for datavolume {name} (phase={phase})")]
    DataVolumeTimeout { name: String, phase: String },
    #[error("VM {name} has a guest NIC binding that bypasses the pod network policy: {}", .findings.join("; "))]
    UnsafeNicBinding { name: String, findings: Vec<String> },
    #[error("server returned another resource name")]
    ReceiptName,
    #[error("server returned no matching resource namespace")]
    ReceiptNamespace,
    #[error("server returned no resource UID")]
    ReceiptUid,
    #[error("cleanup receipt has no matching namespace and UID")]
    CleanupReceipt,
    #[error("warm VM revision changed before eviction")]
    WarmRevisionChanged,
    #[error("owned resource {name} is still present or terminating")]
    StillPresent { name: String },
    #[error("VMI readiness does not belong to the held VM UID")]
    VmiOwnerMismatch,
    #[error("VMI {name} entered Failed phase")]
    VmiFailed { name: String },
    #[error("timeout waiting for VMI {name} to become Ready")]
    VmiReadyTimeout { name: String },
    #[error("VMI {name} reports no interface address yet")]
    NoInterfaceAddress { name: String },
    #[error("timeout waiting for VMI {name} to stop")]
    VmiStopTimeout { name: String },
    #[error("VirtualMachineSnapshot {name} failed")]
    SnapshotFailed { name: String },
    #[error("timeout waiting for VirtualMachineSnapshot {name}")]
    SnapshotTimeout { name: String },
    #[error("no PVC found for VirtualMachineSnapshot {name}")]
    SnapshotPvcMissing { name: String },
    #[error("timeout waiting for export pod {name}")]
    ExportPodTimeout { name: String },
    #[error("export pod produced empty output for pvc {pvc}")]
    EmptyExport { pvc: String },
    #[error("invalid warm pool digest annotation: {0}")]
    WarmDigest(#[from] DigestError),
    #[error("allocation requires scoped resource receipts")]
    UnscopedReceipts,
    #[error("held VM is absent")]
    HeldVmAbsent,
    #[error("held VM was replaced")]
    HeldVmReplaced,
    #[error("cold create or resource ownership remains unresolved; manual ownership reconciliation is required")]
    OwnershipUnresolved,
    #[error("replacement resource shares an original custody dependency; manual reconciliation is required")]
    ReplacementShared,
    #[error("warm claim outcome is unknown while its original disk remains")]
    ClaimUnknownWithDisk,
    #[error("warm claim marker has conflicting ownership labels")]
    ClaimLabelConflict,
    #[error("warm claim outcome remains unknown at its original revision")]
    ClaimUnknownAtRevision,
    #[error("warm claim requires a pre-effect receipt collector")]
    NoReceiptCollector,
    #[error("warm claim response did not confirm its exact effect")]
    ClaimUnconfirmed,
    #[error("warm claim reconciliation remains pending: {inspect}")]
    ClaimPending {
        #[source]
        source: kube::Error,
        inspect: Box<KubeVirtOpsError>,
    },
    #[error("VMI relabel does not belong to claimed VM UID")]
    VmiRelabelForeign,
    #[error("VMI claim relabel has no revision fence")]
    VmiRelabelNoRevision,
}

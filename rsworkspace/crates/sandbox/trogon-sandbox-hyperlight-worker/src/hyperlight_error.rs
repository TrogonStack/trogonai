use trogon_sandbox_contract::{ArtifactType, SandboxIdError};

use crate::{ComponentPathError, HandleStoreError, HypervisorError, SnapshotTransportError};

/// Why the Hyperlight provider refused or failed a lease operation.
#[derive(Debug, thiserror::Error)]
pub enum HyperlightError {
    /// The lease still holds a native handle from an earlier grant.
    #[error("this lease still retains a native handle; settle cleanup before reallocating")]
    Retained,
    /// A sealed Wasm guest has no process environment to receive `spec.env`.
    #[error("spec.env is not supported on the hyperlight provider")]
    Env,
    /// The workspace is a repository clone.
    #[error("clone workspaces are not supported on the hyperlight provider (guest is a sealed Wasm component)")]
    CloneWorkspace,
    /// The workspace is a host directory.
    #[error("host_mount workspaces are not supported on the hyperlight provider")]
    HostMountWorkspace,
    /// The workspace is an S3 volume.
    #[error("s3_volume workspaces are not supported on the hyperlight provider")]
    S3Workspace,
    /// A snapshot image is not a Hyperlight snapshot.
    #[error("hyperlight provider only restores HyperlightSnapshotV1 artifacts, got {0:?}")]
    RestoreArtifactType(ArtifactType),
    /// A snapshot target is not a Hyperlight snapshot.
    #[error("hyperlight provider only publishes HyperlightSnapshotV1 artifacts, got {0:?}")]
    PublishArtifactType(ArtifactType),
    /// The broker left a snapshot tag unresolved.
    #[error("broker must resolve SnapshotTag to a digest-pinned Snapshot before dispatch (got artifact_type {0:?})")]
    UnresolvedSnapshotTag(ArtifactType),
    /// A provider-native snapshot was requested.
    #[error("native provider snapshots are not valid on the hyperlight provider")]
    NativeSnapshot,
    /// `spec.image` does not resolve to a component.
    #[error(transparent)]
    Component(#[from] ComponentPathError),
    /// The lease id cannot name a sandbox.
    #[error("lease id is not a valid sandbox id: {0}")]
    SandboxId(#[from] SandboxIdError),
    /// Acquiring the guest failed.
    #[error("hyperlight sandbox acquisition failed: {0}")]
    Acquire(#[source] Box<HyperlightError>),
    /// No lease with a guest is held under this id.
    #[error("no active lease {0} on hyperlight provider")]
    NoLease(String),
    /// The hypervisor failed.
    #[error(transparent)]
    Hypervisor(#[from] HypervisorError),
    /// The retained handle store refused.
    #[error(transparent)]
    Store(#[from] HandleStoreError),
    /// Moving a snapshot through the registry failed.
    #[error(transparent)]
    Snapshot(#[from] SnapshotTransportError),
    /// A scratch directory could not be created.
    #[error("cannot create a scratch directory: {0}")]
    Scratch(#[source] std::io::Error),
    /// A blocking task did not finish.
    #[error("blocking task failed: {0}")]
    Join(#[source] tokio::task::JoinError),
}

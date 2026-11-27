use std::path::PathBuf;

use trogon_sandbox_contract::{HostMountAccess, LeaseRequest, NetworkPolicy, WorkspaceSource};

use crate::{GuestNetwork, HlukRuntime, HlukSpecError, LeaseWorkspace};

/// A granted lease: no guest stays up between commands, only what each `hluk run` needs to restore one.
#[derive(Debug)]
pub struct HlukLease {
    pub runtime: HlukRuntime,
    /// The host directory mounted at `/workspace`, the only state that outlives a command.
    pub workspace: PathBuf,
    pub kind: LeaseWorkspace,
    pub network: GuestNetwork,
}

impl HlukLease {
    /// The lease `request` asks for, with a worker-created workspace at `scratch` when it asks for an empty one.
    pub fn plan(request: &LeaseRequest, scratch: PathBuf) -> Result<Self, HlukSpecError> {
        let spec = &request.spec;
        let runtime = HlukRuntime::from_image(spec.image())?;
        if !spec.mounts().is_empty() || !spec.drives().is_empty() {
            return Err(HlukSpecError::Mounts);
        }
        if !spec.exposed_ports().is_empty() {
            return Err(HlukSpecError::Ports);
        }
        if spec.user().is_some() {
            return Err(HlukSpecError::User);
        }
        if spec.fs_policy().is_some() {
            return Err(HlukSpecError::FsPolicy);
        }
        if !spec.secret_bindings().is_empty() {
            return Err(HlukSpecError::SecretBindings);
        }
        if spec.wasi_capabilities().is_some() {
            return Err(HlukSpecError::WasiCapabilities);
        }
        if spec.snapshot_out().is_some() {
            return Err(HlukSpecError::SnapshotOut);
        }
        let network = match spec.network_policy() {
            None | Some(NetworkPolicy::DenyAll) => GuestNetwork::Denied,
            Some(NetworkPolicy::AllowAll) => GuestNetwork::Open,
            Some(NetworkPolicy::UserDefined { .. }) => return Err(HlukSpecError::NetworkPolicy),
        };
        let (workspace, kind) = match spec.workspace() {
            WorkspaceSource::Empty(_) => (scratch, LeaseWorkspace::Scratch),
            WorkspaceSource::HostMount {
                path,
                access: HostMountAccess::ReadOnly,
            } => (PathBuf::from(path), LeaseWorkspace::CallerReadOnly),
            WorkspaceSource::HostMount { .. } => return Err(HlukSpecError::Workspace("a writable host mount")),
            WorkspaceSource::Clone { .. } => return Err(HlukSpecError::Workspace("a git clone")),
            WorkspaceSource::S3Volume { .. } => return Err(HlukSpecError::Workspace("an S3 volume")),
        };
        Ok(Self {
            runtime,
            workspace,
            kind,
            network,
        })
    }
}

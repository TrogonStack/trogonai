use std::path::{Component, Path, PathBuf};

use trogon_sandbox_contract::{HostMountAccess, LeaseRequest, NetworkPolicy, WorkspaceSource};
use trogon_sandbox_protocol::resolve_exec_cwd;

use crate::constants::WORKSPACE_GUEST_DIR;
use crate::{GuestNetwork, HlukRuntime, HlukSpecError, WorkspaceAccess, WorkspaceOwner};

/// A granted lease: no guest stays up between commands, only what each `hluk run` needs to restore one.
#[derive(Debug)]
pub struct HlukLease {
    pub runtime: HlukRuntime,
    /// The host directory mounted at [`WORKSPACE_GUEST_DIR`], the only state that outlives a command.
    pub workspace: PathBuf,
    pub access: WorkspaceAccess,
    pub owner: WorkspaceOwner,
    pub network: GuestNetwork,
}

impl HlukLease {
    /// The lease `request` asks for, with a worker-created workspace at `scratch` when it asks for an empty one.
    pub fn plan(request: &LeaseRequest, scratch: PathBuf) -> Result<Self, HlukSpecError> {
        let spec = &request.spec;
        let runtime = HlukRuntime::from_image(spec.image())?;
        let refusal = if !spec.mounts().is_empty() || !spec.drives().is_empty() {
            Some(HlukSpecError::Mounts)
        } else if !spec.exposed_ports().is_empty() {
            Some(HlukSpecError::Ports)
        } else if spec.user().is_some() {
            Some(HlukSpecError::User)
        } else if spec.fs_policy().is_some() {
            Some(HlukSpecError::FsPolicy)
        } else if !spec.secret_bindings().is_empty() {
            Some(HlukSpecError::SecretBindings)
        } else if spec.wasi_capabilities().is_some() {
            Some(HlukSpecError::WasiCapabilities)
        } else if spec.snapshot_out().is_some() {
            Some(HlukSpecError::SnapshotOut)
        } else {
            None
        };
        if let Some(refusal) = refusal {
            return Err(refusal);
        }
        let network = match spec.network_policy() {
            None | Some(NetworkPolicy::DenyAll) => GuestNetwork::Denied,
            Some(NetworkPolicy::AllowAll) => GuestNetwork::Open,
            Some(NetworkPolicy::UserDefined { .. }) => return Err(HlukSpecError::NetworkPolicy),
        };
        let (workspace, access, owner) = match spec.workspace() {
            WorkspaceSource::Empty(_) => (scratch, WorkspaceAccess::ReadWrite, WorkspaceOwner::Worker),
            WorkspaceSource::HostMount {
                path,
                access: HostMountAccess::ReadOnly,
            } => (PathBuf::from(path), WorkspaceAccess::ReadOnly, WorkspaceOwner::Caller),
            WorkspaceSource::HostMount { .. } => return Err(HlukSpecError::Workspace("a writable host mount")),
            WorkspaceSource::Clone { .. } => return Err(HlukSpecError::Workspace("a git clone")),
            WorkspaceSource::S3Volume { .. } => return Err(HlukSpecError::Workspace("an S3 volume")),
        };
        Ok(Self {
            runtime,
            workspace,
            access,
            owner,
            network,
        })
    }

    /// The host file behind guest `path`, which must lie inside the workspace. A relative path resolves against it.
    pub fn host_path(&self, path: &str) -> Result<PathBuf, String> {
        let guest = resolve_exec_cwd(WORKSPACE_GUEST_DIR, Some(path));
        let inside = Path::new(&guest).strip_prefix(WORKSPACE_GUEST_DIR).map_err(|_| {
            format!("{path} is outside {WORKSPACE_GUEST_DIR}, the only guest directory that outlives a command")
        })?;
        if inside.components().any(|part| !matches!(part, Component::Normal(_))) {
            return Err(format!("{path} must not leave {WORKSPACE_GUEST_DIR}"));
        }
        Ok(self.workspace.join(inside))
    }
}

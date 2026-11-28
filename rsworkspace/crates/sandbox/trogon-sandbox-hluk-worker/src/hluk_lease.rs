use std::path::PathBuf;

use trogon_sandbox_contract::{HostMountAccess, LeaseRequest, NetworkPolicy, WorkspaceSource};

use crate::{GuestDir, GuestNetwork, HlukRuntime, HlukSpecError, LeaseWorkspace};

/// A granted lease: no guest stays up between commands, only what each `hluk run` needs to restore one.
#[derive(Debug)]
pub struct HlukLease {
    pub runtime: HlukRuntime,
    /// The host directory the worker owns for this lease and removes when it ends.
    pub dir: PathBuf,
    /// The host directory mounted at `/workspace`: under `dir` unless the caller mounts its own.
    pub workspace: PathBuf,
    pub kind: LeaseWorkspace,
    pub network: GuestNetwork,
}

impl HlukLease {
    /// The lease `request` asks for, keeping its worker-owned directories under `dir`.
    pub fn plan(request: &LeaseRequest, dir: PathBuf) -> Result<Self, HlukSpecError> {
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
            WorkspaceSource::Empty(_) => (dir.join("workspace"), LeaseWorkspace::Scratch),
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
            dir,
            workspace,
            kind,
            network,
        })
    }

    /// The host directory mounted at `dir`.
    pub fn host_dir(&self, dir: GuestDir) -> PathBuf {
        match dir {
            GuestDir::Workspace => self.workspace.clone(),
            GuestDir::Tmp => self.dir.join("tmp"),
        }
    }

    /// Creates the host directories the worker owns for this lease.
    pub async fn create_dirs(&self) -> std::io::Result<()> {
        tokio::fs::create_dir_all(self.host_dir(GuestDir::Tmp)).await?;
        if self.kind == LeaseWorkspace::Scratch {
            tokio::fs::create_dir_all(&self.workspace).await?;
        }
        Ok(())
    }

    /// How `dir` is mounted in the guest: read-only only for a caller's workspace.
    pub fn read_only(&self, dir: GuestDir) -> bool {
        dir == GuestDir::Workspace && self.kind == LeaseWorkspace::CallerReadOnly
    }
}

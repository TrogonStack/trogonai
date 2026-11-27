//! Maps a [`LeaseRequest`] into the providers' [`SandboxSpec`].

mod constants;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use trogon_sandbox_contract::{LeaseId, LeaseRequest, SandboxSpec as LeaseSandboxSpec, WorkspaceSource};

use crate::image_label::sandbox_image_label;
use crate::{
    CloneCredentials, CommitSha, EgressPolicy, GitProvider, HostMount, ImageRef, LeaseSpecError, NoopArtifactSink,
    RepoRef, SandboxSpec,
};

pub use constants::DEFAULT_LEASE_CPUS;

/// Cores for the container. An explicit `vcpu_millis` is a size request;
/// without one the lease gets [`DEFAULT_LEASE_CPUS`]. `cpu_seconds` is a
/// CPU-time budget, not a core count, so it never sizes the container.
pub fn lease_cpus(spec: &LeaseSandboxSpec) -> f32 {
    match spec.vcpu_millis() {
        Some(millis) => millis as f32 / 1000.0,
        None => DEFAULT_LEASE_CPUS,
    }
}

/// How long `req`'s sandbox may live. Zero is a lease with no deadline: it
/// asked for `no_ttl` and ends only on release or the broker's reapers.
pub fn lease_timeout(req: &LeaseRequest) -> Duration {
    if req.spec.no_ttl().is_unbounded() {
        Duration::ZERO
    } else {
        Duration::from_secs(u64::from(req.ttl_seconds))
    }
}

#[tracing::instrument(skip_all, fields(tenant = %req.tenant, lease_id = %lease_id))]
pub fn translate_lease_request_to_sandbox_spec(
    req: &LeaseRequest,
    lease_id: &LeaseId,
) -> Result<SandboxSpec, LeaseSpecError> {
    let lease_spec = &req.spec;
    let image = ImageRef::new(sandbox_image_label(lease_spec.image()));

    let (resolved_sha, base_sha, clone, host_mount) = match lease_spec.workspace() {
        WorkspaceSource::Clone {
            url,
            sha,
            base_sha,
            auth,
        } => {
            let resolved_sha = CommitSha::new(sha).map_err(LeaseSpecError::InvalidResolvedSha)?;
            let base_sha = base_sha
                .as_deref()
                .map(CommitSha::new)
                .transpose()
                .map_err(LeaseSpecError::InvalidBaseSha)?;
            let credentials = CloneCredentials {
                clone_url: url.clone(),
                auth: auth.clone(),
            };
            (resolved_sha, base_sha, Some(credentials), None)
        }
        WorkspaceSource::HostMount { path, access } => {
            if path.is_empty() {
                return Err(LeaseSpecError::HostMountPathEmpty);
            }
            let path_buf = PathBuf::from(path);
            if !path_buf.is_absolute() {
                return Err(LeaseSpecError::HostMountPathNotAbsolute { path: path.clone() });
            }
            let mount = HostMount {
                path: path_buf,
                access: *access,
            };
            (CommitSha::placeholder(), None, None, Some(mount))
        }
        WorkspaceSource::Empty(_) => (CommitSha::placeholder(), None, None, None),
        WorkspaceSource::S3Volume { .. } => return Err(LeaseSpecError::S3VolumeUnsupported),
    };

    let repo = RepoRef {
        provider: GitProvider::GitHub,
        owner: "sandbox".to_string(),
        name: lease_id.as_str().to_lowercase(),
    };

    Ok(SandboxSpec {
        image,
        cpu: lease_cpus(lease_spec),
        memory_mib: lease_spec.memory_mib().get(),
        disk_mib: lease_spec.disk_mib(),
        timeout: lease_timeout(req),
        repo,
        resolved_sha,
        base_sha,
        tools: vec![],
        artifact_sink: Arc::new(NoopArtifactSink),
        egress_policy: EgressPolicy::DenyAll,
        network_policy: lease_spec.network_policy().cloned(),
        clone,
        host_mount,
        extra_binds: vec![],
        boot_mode: lease_spec.boot_mode(),
        working_dir: lease_spec.working_dir().map(str::to_string),
        user: lease_spec.user().map(str::to_string),
        privileged: lease_spec.privileged(),
        exposed_ports: lease_spec.exposed_ports().iter().map(|port| port.get()).collect(),
        env: lease_spec.env().clone(),
        lease_id: Some(lease_id.clone()),
    })
}

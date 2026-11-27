//! What a lease may ask of this provider.
//!
//! The broker does not validate against this worker's capabilities, so this is the
//! authoritative refusal. Each rejection names the field and the reason, because the tenant
//! sees the message verbatim as an `InvalidSpec`.

use std::collections::BTreeMap;

use trogon_sandbox_contract::{
    ArtifactType, CapabilityNeed, CapabilityRequirements, Digest, DomainRuleAction, LeaseRequest, NetworkPolicy,
    OciArtifactRef, SandboxImage, SecretBinding, SecretMode, SecretTarget, WorkspaceSource,
};

use crate::capability_check::{PolicyRequirements, ProfileSatisfies};
use crate::config::{FirecrackerConfig, MemoryMib};
use crate::error::ProviderError;

mod constants;
pub use constants::DEFAULT_WORKING_DIR;

/// What the guest starts from.
#[derive(Debug, Clone, Eq, PartialEq)]
pub enum Origin {
    /// A cold boot of the base rootfs.
    Boot,
    /// A checkpoint this worker took of another lease, resumed where it was paused. The
    /// guest keeps the memory size it was taken with.
    Snapshot(Digest),
}

/// A lease request reduced to what this provider will actually act on.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct AcceptedSpec {
    pub tenant: trogon_sandbox_contract::TenantId,
    pub origin: Origin,
    pub memory: MemoryMib,
    pub registry_image: Option<trogon_sandbox_contract::OciImageRef>,
    pub restore: Option<OciArtifactRef>,
    pub working_dir: String,
    pub working_dir_explicit: bool,
    pub env: BTreeMap<String, String>,
    pub network_policy: Option<NetworkPolicy>,
    pub network_policy_request: Option<NetworkPolicy>,
    pub snapshot_out: Option<trogon_sandbox_contract::SnapshotPolicy>,
}

fn refuse(message: impl Into<String>) -> ProviderError {
    ProviderError::InvalidSpec(message.into())
}

pub fn accept(config: &FirecrackerConfig, req: &LeaseRequest) -> Result<AcceptedSpec, ProviderError> {
    let spec = &req.spec;
    if !crate::profile::PROFILE.satisfies(&secret_binding_requirements(spec.secret_bindings().bindings())) {
        return Err(refuse(
            "secret_bindings require features unavailable on the Firecracker worker",
        ));
    }
    if let Some(policy) = spec.fs_policy()
        && !crate::profile::PROFILE.satisfies(&CapabilityRequirements::default().with_fs_policy(policy))
    {
        return Err(refuse(
            "fs_policy requires features unavailable on the Firecracker worker",
        ));
    }
    if spec
        .snapshot_out()
        .is_some_and(|policy| policy.artifact_type != ArtifactType::FirecrackerSnapshotV1)
    {
        return Err(refuse("snapshot_out must use FirecrackerSnapshotV1"));
    }
    if spec.snapshot_out().is_some()
        && (config.released_snapshots.max_bytes == 0
            || config.released_snapshots.max_records == 0
            || config.released_snapshots.export_ttl.is_zero())
    {
        return Err(refuse("released snapshot retention is disabled"));
    }

    match spec.image() {
        SandboxImage::Registry { reference, .. } => {
            let wanted = reference.allowlist_string();
            let served = config.default_image.allowlist_string();
            if wanted != served && config.oci_rootfs.is_none() {
                return Err(refuse(format!(
                    "the firecracker provider boots one preinstalled root filesystem and cannot pull images: \
                     requested {wanted:?}, but only {served:?} is served"
                )));
            }
        }
        SandboxImage::Snapshot { reference, .. } if reference.artifact_type == ArtifactType::FirecrackerSnapshotV1 => {}
        _ => {
            return Err(refuse(
                "the firecracker provider accepts registry images and only its own snapshots (FirecrackerSnapshotV1)",
            ));
        }
    }

    if !spec.mounts().is_empty() {
        return Err(refuse(
            "remote mounts are not supported on the firecracker provider: the guest has no network device and no FUSE",
        ));
    }
    if !spec.drives().is_empty() {
        return Err(refuse(
            "persistent drives are not supported on the firecracker provider: the root filesystem is a per-lease copy",
        ));
    }
    if !spec.exposed_ports().is_empty() {
        return Err(refuse(
            "exposed_ports are not supported on the firecracker provider: the guest has no network device to publish a port from",
        ));
    }
    if config.legacy_network.is_some()
        && let Some(policy) = spec
            .network_policy()
            .filter(|policy| !matches!(policy, NetworkPolicy::DenyAll))
    {
        if !config.intercepts_tls() {
            crate::network::enforceable(policy).map_err(refuse)?;
        }
        if spec.snapshot_out().is_some() || matches!(spec.image(), SandboxImage::Snapshot { .. }) {
            return Err(refuse(
                "a restored microVM cannot gain a network device through the legacy network helper; networked snapshots require host-links mode",
            ));
        }
    }
    match spec.network_policy() {
        None | Some(NetworkPolicy::DenyAll) => {}
        Some(NetworkPolicy::AllowAll) if !config.has_network() => {
            return Err(refuse(
                "network_policy allow_all is not supported on the firecracker provider: the guest has no network device, so egress is always denied; configure --netns-helper or --host-links to enable egress",
            ));
        }
        Some(NetworkPolicy::UserDefined { .. }) if !config.has_network() => {
            return Err(refuse(
                "a user defined network_policy is not supported on the firecracker provider: the guest has no network device, so egress is always denied; configure --netns-helper or --host-links to enable egress",
            ));
        }
        Some(_) => {}
    }
    if let Some(policy) = spec.network_policy() {
        if crate::host_network::forwards_requests(policy) && !config.forwards_requests() {
            return Err(refuse(
                "request forwarding rules need host-links networking and --oidc-signing-key-pem-file: only then does the lease proxy have an identity signer and a guest that trusts it",
            ));
        }
        if substitutes_secrets(policy) && !config.injects_credentials() {
            return Err(refuse(
                "credential injection needs --credential-broker-nats-url with --host-links or --netns-helper networking: only then does the guest trust the egress proxy's CA and the proxy have a credential source",
            ));
        }
        let mut profile = crate::profile::PROFILE.clone();
        crate::profile::with_network(&mut profile, config);
        let requirements = CapabilityRequirements::default().with_network_policy(policy);
        if !profile.satisfies(&requirements) {
            return Err(refuse(
                "network policy requires features unavailable on the Firecracker worker",
            ));
        }
    }
    if spec.user().is_some() {
        return Err(refuse(
            "user overrides are not supported on the firecracker provider: commands choose their user per request",
        ));
    }
    match spec.workspace() {
        WorkspaceSource::Empty(_) => {}
        WorkspaceSource::Clone { .. } => {
            return Err(refuse(
                "clone workspaces are not supported on the firecracker provider: the guest has no network to clone over",
            ));
        }
        WorkspaceSource::HostMount { .. } => {
            return Err(refuse(
                "host_mount workspaces are not supported on the firecracker provider: the guest cannot see host paths",
            ));
        }
        WorkspaceSource::S3Volume { .. } => {
            return Err(refuse(
                "s3 volume workspaces are not supported on the firecracker provider: the guest has no network to sync over",
            ));
        }
    }

    let memory = MemoryMib::new(spec.memory_mib().get()).map_err(|e| refuse(e.to_string()))?;

    Ok(AcceptedSpec {
        tenant: req.tenant.clone(),
        origin: match spec.image() {
            SandboxImage::Snapshot { reference, .. } => Origin::Snapshot(reference.digest.clone()),
            _ => Origin::Boot,
        },
        snapshot_out: spec.snapshot_out().cloned(),
        memory,
        registry_image: match spec.image() {
            SandboxImage::Registry { reference, pull_secret } if *reference != config.default_image => {
                if pull_secret.is_some() {
                    return Err(refuse(
                        "OCI pull_secret references are not supported by the firecracker worker; configure worker registry credentials",
                    ));
                }
                Some(reference.clone())
            }
            _ => None,
        },
        restore: match spec.image() {
            SandboxImage::Snapshot { reference, .. } => Some(reference.clone()),
            _ => None,
        },
        working_dir: spec
            .working_dir()
            .map_or_else(|| DEFAULT_WORKING_DIR.to_string(), str::to_string),
        working_dir_explicit: spec.working_dir().is_some(),
        env: spec.env().clone(),
        network_policy: match spec.network_policy() {
            Some(NetworkPolicy::DenyAll) | None => None,
            Some(policy) => Some(policy.clone()),
        },
        network_policy_request: spec.network_policy().cloned(),
    })
}

/// What secret bindings need from a worker. Masking additionally needs TLS termination to swap
/// sentinels back, and masking a file needs a worker that can bind a fake copy over it.
fn secret_binding_requirements(bindings: &[SecretBinding]) -> CapabilityRequirements {
    let mut requirements = CapabilityRequirements::default();
    if bindings.is_empty() {
        return requirements;
    }
    requirements.needs_secret_bindings = CapabilityNeed::Required;
    if bindings.iter().any(|binding| binding.mode() == SecretMode::Mask) {
        requirements.needs_tls_intercept = CapabilityNeed::Required;
        requirements.needs_secret_substitution = CapabilityNeed::Required;
    }
    if bindings
        .iter()
        .any(|binding| binding.mode() == SecretMode::Mask && matches!(binding.target(), SecretTarget::File(_)))
    {
        requirements.needs_masked_files = CapabilityNeed::Required;
    }
    requirements
}

/// Whether `policy` swaps credentials into requests.
fn substitutes_secrets(policy: &NetworkPolicy) -> bool {
    match policy {
        NetworkPolicy::UserDefined { rules, .. } => rules
            .iter()
            .any(|rule| matches!(rule.action, DomainRuleAction::Transform(_))),
        NetworkPolicy::AllowAll | NetworkPolicy::DenyAll => false,
    }
}

#[cfg(test)]
mod tests;

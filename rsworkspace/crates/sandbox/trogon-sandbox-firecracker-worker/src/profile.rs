use std::sync::LazyLock;

use trogon_sandbox_contract::{
    ArtifactType, CapabilitySupport, DormancyCapability, EgressEnforcementLevel, IngressCapability,
    IsolationCapability, IsolationTier, LifecycleCapability, PerformanceProfile, ProviderCapabilityProfile,
    ProviderKind, SnapshotCapability, VolumeCapability,
};

/// Honest capability profile for a Firecracker worker that boots every guest without a
/// network device.
///
/// Each lease is one KVM guest with its own kernel, so the isolation boundary is a hardware
/// virtualization one. With no network device there is no path out to police, so egress
/// enforcement is `Mandatory` and no policy can be bypassed. The flip side is that nothing
/// needing a network (clones, exposed ports, allow-lists) is offered.
///
/// Not done, and therefore reported as absent: warm pools and volumes. The worker refuses to
/// start without the jailer unless `--insecure-dev` is given, so a worker serving leases has
/// its VMM confined by a chroot, cgroups and a per-guest UID (the one exception is a worker
/// started with `--insecure-dev`, which runs the VMM as its own user). Snapshots are added by [`for_worker`] only for a
/// worker that keeps them.
pub static PROFILE: LazyLock<ProviderCapabilityProfile> = LazyLock::new(|| {
    let on = CapabilitySupport::Supported;
    let off = CapabilitySupport::Unsupported;
    ProviderCapabilityProfile {
        provider: ProviderKind::Firecracker,
        snapshot: SnapshotCapability {
            can_snapshot: on,
            can_checkpoint_running: on,
            can_restore: on,
            supported_artifact_types: vec![ArtifactType::FirecrackerSnapshotV1],
            layered_deltas: off,
            supports_native_snapshots: off,
            supports_in_place_rollback: off,
            max_snapshot_size_bytes: None,
        },
        volume: VolumeCapability {
            supported_modes: vec![],
            supports_s3_backend: off,
            supports_fuse_mounts: off,
        },
        isolation: IsolationCapability {
            default_tier: IsolationTier::MicroVm,
            supported_tiers: vec![IsolationTier::MicroVm],
            egress_enforcement: EgressEnforcementLevel::Mandatory,
            // The guest is a whole VM with its own kernel, so root inside it grants nothing
            // on the host.
            supports_privileged: on,
            supports_sandbox_users: on,
            supports_interactive_stdin: on,
            supports_named_processes: off,
            supports_process_readiness: off,
            supports_graceful_stop: off,
            supports_command_restart: off,
            supports_command_cancellation: off,
            supports_command_launch_cancellation: off,
            supports_port_preview: off,
            supports_expose_after_create: off,
            supports_evidence_streaming: on,
            supports_output_streaming: on,
            supports_domain_allowlist: off,
            supports_tls_intercept: off,
            supports_secret_substitution: off,
            supports_secret_bindings: off,
            supports_masked_files: off,
            supports_fs_policy: off,
            supports_file_ops: off,
            supports_file_search: off,
            supports_file_archive: off,
            supports_output_redaction: off,
            supports_terminal: on,
            resource_limits: trogon_sandbox_contract::ResourceLimitMechanism::None,
            egress: trogon_sandbox_contract::EgressCapability::DENY_ONLY,
            supports_hardening_overrides: off,
            supports_change_tracking: off,
            supports_file_watch: off,
            supports_persistent_runtime: off,
            supports_dir_transfer: off,
            supports_browser: off,
            supports_network_policy_update: on,
            supports_sandbox_groups: on,
        },
        lifecycle: LifecycleCapability {
            supports_warm_pool: off,
            max_warm_pool_size: None,
            supports_named_tags: off,
            supports_prewarm_per_tenant: off,
            supports_pause_resume: off,
            supports_native_auto_stop: off,
            max_lease_lifetime_seconds: None,
            dormancy: DormancyCapability::NONE,
        },
        performance: PerformanceProfile {
            // [!] An estimate, not a measurement: a boot plus the agent's readiness probe on
            // a rootfs copy. Replace with an observed p50 before routing relies on it.
            cold_start_p50_ms: 2000,
            cold_restore_p50_ms: 0,
            max_concurrent_sandboxes: None,
        },
        resources: Default::default(),
        placement: Default::default(),
        ingress: IngressCapability {
            max_exposed_ports: Some(0),
        },
    }
});

/// The profile of a worker that gives leases a network device on request.
///
/// Egress stays `Mandatory`: the namespace firewall drops everything the guest sends except
/// TCP to its own egress proxy, so the lease's policy is the only way out. Domain
/// allow-lists are enforced by that proxy. TLS interception and credential injection are
/// added per worker by [`with_network`], because they depend on how the worker is configured.
pub static NETWORKED_PROFILE: LazyLock<ProviderCapabilityProfile> = LazyLock::new(|| {
    let mut profile = PROFILE.clone();
    profile.isolation.supports_domain_allowlist = CapabilitySupport::Supported;
    profile.isolation.egress = trogon_sandbox_contract::EgressCapability::PROXIED;
    profile
});

/// Adds what `config` lets a lease do with a network device. The proxy can only terminate TLS
/// for a guest that trusts its CA, which host-links mode installs before the lease is ready,
/// and can only inject credentials when a broker is configured. The legacy helper mode does
/// both only with a broker, which puts its leases on an in-process proxy; without one it
/// does neither.
pub fn with_network(profile: &mut ProviderCapabilityProfile, config: &crate::config::FirecrackerConfig) {
    if config.has_network() {
        profile.isolation.supports_domain_allowlist = CapabilitySupport::Supported;
        profile.isolation.egress = trogon_sandbox_contract::EgressCapability::PROXIED;
    }
    profile.isolation.supports_tls_intercept = config.intercepts_tls().into();
    profile.isolation.supports_secret_substitution = config.injects_credentials().into();
}

/// The profile a worker advertises. One started with a snapshot dir checkpoints running
/// leases without stopping them and restores new leases from those checkpoints, but only
/// leases without a network device, and only on the host that took the snapshot: nothing is
/// pushed to a registry.
pub fn for_worker(networked: bool, snapshots: bool) -> ProviderCapabilityProfile {
    let mut profile = if networked {
        NETWORKED_PROFILE.clone()
    } else {
        PROFILE.clone()
    };
    if !snapshots {
        profile.snapshot.can_snapshot = CapabilitySupport::Unsupported;
        profile.snapshot.can_checkpoint_running = CapabilitySupport::Unsupported;
        profile.snapshot.can_restore = CapabilitySupport::Unsupported;
        profile.snapshot.supported_artifact_types.clear();
    }
    if snapshots {
        profile.snapshot.can_snapshot = CapabilitySupport::Supported;
        profile.snapshot.can_checkpoint_running = CapabilitySupport::Supported;
        profile.snapshot.can_restore = CapabilitySupport::Supported;
        profile.snapshot.supported_artifact_types = vec![ArtifactType::FirecrackerSnapshotV1];
        // Measured on billy for a 256 MiB guest: sparse rootfs copy, snapshot load, and the
        // agent answering with its new identity.
        profile.performance.cold_restore_p50_ms = 250;
    }
    profile
}

#[cfg(test)]
mod tests;

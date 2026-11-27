use trogon_sandbox_contract::{
    ArtifactType, CapabilitySupport, DormancyCapability, EgressCapability, EgressEnforcementLevel, IsolationCapability,
    IsolationTier, LifecycleCapability, PerformanceProfile, ProviderCapabilityProfile, ProviderKind,
    ResourceCapability, ResourceLimitMechanism, ResourceRange, SnapshotCapability, VolumeCapability, VolumeMode,
};

use crate::constants::{MAX_VCPU_MILLIS, MIN_VCPU_MILLIS};

/// The profile every Kubernetes microVM worker starts from, before its
/// configured RuntimeClasses decide the isolation tiers.
pub fn profile() -> ProviderCapabilityProfile {
    let off = CapabilitySupport::Unsupported;
    let on = CapabilitySupport::Supported;
    ProviderCapabilityProfile {
        provider: ProviderKind::K8sMicroVm,
        snapshot: SnapshotCapability {
            can_snapshot: on,
            can_checkpoint_running: on,
            can_restore: on,
            supported_artifact_types: vec![ArtifactType::SandboxRootfsV1],
            layered_deltas: off,
            supports_native_snapshots: off,
            supports_in_place_rollback: on,
            max_snapshot_size_bytes: None,
        },
        volume: VolumeCapability {
            supported_modes: vec![VolumeMode::ReadOnly, VolumeMode::ReadWrite],
            supports_s3_backend: on,
            // Unadvertised on purpose. The FUSE device mount is reachable from the
            // lease path, but it is granted only when the resolved RuntimeClass
            // name looks like kata or gvisor, and that name is operator
            // configuration, not a live read of the cluster. Advertising it
            // would let the broker route FUSE leases to a cluster that may not
            // have such a RuntimeClass, where they would fail late at launch.
            supports_fuse_mounts: off,
        },
        isolation: IsolationCapability {
            default_tier: IsolationTier::UserSpaceKernel,
            supported_tiers: vec![IsolationTier::UserSpaceKernel, IsolationTier::MicroVm],
            egress_enforcement: EgressEnforcementLevel::BestEffort,
            // Privileged exec needs a kata or gvisor RuntimeClass, which is cluster
            // deploy-time configuration this code cannot verify, so it is not
            // advertised by default.
            supports_privileged: off,
            // User and group management runs useradd, groupadd, usermod and gpasswd
            // in the pod, so it needs no cluster-specific RuntimeClass.
            supports_sandbox_users: on,
            supports_interactive_stdin: on,
            supports_named_processes: off,
            supports_process_readiness: off,
            supports_graceful_stop: on,
            supports_command_restart: on,
            supports_command_cancellation: off,
            supports_command_launch_cancellation: off,
            supports_port_preview: off,
            supports_expose_after_create: off,
            supports_evidence_streaming: on,
            supports_output_streaming: off,
            // The pod's `NetworkPolicy` matches IP ranges only. Domain rules would
            // need the egress proxy sidecar, which the worker refuses, and with it
            // TLS interception and secret substitution.
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
            resource_limits: ResourceLimitMechanism::None,
            egress: EgressCapability {
                deny_all: on,
                host_allowlist: off,
                cidr_allowlist: on,
                proxy_mediated: off,
            },
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
            supports_warm_pool: on,
            max_warm_pool_size: Some(16),
            supports_named_tags: on,
            supports_prewarm_per_tenant: on,
            supports_pause_resume: off,
            supports_native_auto_stop: off,
            max_lease_lifetime_seconds: None,
            dormancy: DormancyCapability::NONE,
        },
        performance: PerformanceProfile {
            cold_start_p50_ms: 5000,
            cold_restore_p50_ms: 10000,
            max_concurrent_sandboxes: Some(4),
        },
        resources: ResourceCapability {
            // Pods take fractional CPU as both the request and the limit.
            vcpu_millis: ResourceRange::new(MIN_VCPU_MILLIS, MAX_VCPU_MILLIS).ok(),
            ..ResourceCapability::default()
        },
        placement: Default::default(),
        ingress: Default::default(),
    }
}

/// The isolation tiers whose leases would run under a non-empty
/// RuntimeClass. `runtime_class_for` falls back to the default class for a
/// tier with no override, so a tier is only as isolated as that resolved
/// class: with neither set, the pod runs under the node's plain container
/// runtime and the tier must not be claimed.
pub fn isolation_tiers_for(
    runtime_class_for: impl Fn(Option<IsolationTier>) -> String,
) -> (IsolationTier, Vec<IsolationTier>) {
    let mut supported: Vec<IsolationTier> = [IsolationTier::UserSpaceKernel, IsolationTier::MicroVm]
        .into_iter()
        .filter(|tier| !runtime_class_for(Some(*tier)).is_empty())
        .collect();
    // A lease that names no tier gets the default class, so with an empty
    // default it lands on a plain container.
    let untiered_is_container = runtime_class_for(None).is_empty();
    if untiered_is_container {
        supported.insert(0, IsolationTier::Container);
    }
    let default_tier = supported.first().copied().unwrap_or(IsolationTier::Container);
    (default_tier, supported)
}

/// The profile this worker advertises for its configured RuntimeClasses.
pub fn profile_for(runtime_class_for: impl Fn(Option<IsolationTier>) -> String) -> ProviderCapabilityProfile {
    let mut profile = profile();
    let (default_tier, supported_tiers) = isolation_tiers_for(runtime_class_for);
    profile.isolation.default_tier = default_tier;
    profile.isolation.supported_tiers = supported_tiers;
    profile
}

#[cfg(test)]
mod tests;

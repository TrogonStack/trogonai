use trogon_sandbox_contract::{
    ArtifactType, CapabilitySupport, DormancyCapability, EgressCapability, EgressEnforcementLevel, IngressCapability,
    IsolationCapability, IsolationTier, LifecycleCapability, PerformanceProfile, PlacementCapability,
    ProviderCapabilityProfile, ProviderKind, ResourceCapability, ResourceLimitMechanism, ResourceRange,
    SnapshotCapability, VolumeCapability, VolumeMode,
};

use crate::constants::{
    COLD_RESTORE_P50_MS, COLD_START_P50_MS, MAX_VCPU_MILLIS, MIN_VCPU_MILLIS, PROFILE_MAX_CONCURRENT_SANDBOXES,
    PROFILE_MAX_WARM_POOL_SIZE,
};

/// What the KubeVirt worker offers.
///
/// Guest NIC anti-spoofing is delegated to the pod network: the worker reads back every VM it creates and
/// deletes it unless all interfaces are `masquerade` on the pod network. Launch ignores `network_policy`, so no
/// egress restriction is enforced.
pub fn profile() -> ProviderCapabilityProfile {
    let off = CapabilitySupport::Unsupported;
    let on = CapabilitySupport::Supported;
    ProviderCapabilityProfile {
        provider: ProviderKind::KubeVirt,
        snapshot: SnapshotCapability {
            can_snapshot: on,
            can_checkpoint_running: on,
            can_restore: on,
            supported_artifact_types: vec![ArtifactType::KubevirtDiskV1],
            layered_deltas: off,
            max_snapshot_size_bytes: None,
            supports_native_snapshots: off,
            supports_in_place_rollback: off,
        },
        volume: VolumeCapability {
            supported_modes: vec![VolumeMode::ReadOnly, VolumeMode::ReadWrite],
            supports_s3_backend: on,
            supports_fuse_mounts: off,
        },
        isolation: IsolationCapability {
            default_tier: IsolationTier::FullVm,
            supported_tiers: vec![IsolationTier::FullVm],
            egress_enforcement: EgressEnforcementLevel::None,
            supports_privileged: off,
            supports_sandbox_users: off,
            supports_interactive_stdin: off,
            supports_named_processes: off,
            supports_process_readiness: off,
            supports_graceful_stop: off,
            supports_command_restart: off,
            supports_command_cancellation: off,
            supports_command_launch_cancellation: off,
            supports_port_preview: off,
            supports_expose_after_create: off,
            supports_evidence_streaming: on,
            supports_output_streaming: off,
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
            egress: EgressCapability::NONE,
            supports_hardening_overrides: off,
            supports_change_tracking: off,
            supports_file_watch: off,
            supports_persistent_runtime: off,
            supports_dir_transfer: off,
            supports_browser: off,
            supports_network_policy_update: off,
            supports_sandbox_groups: off,
        },
        lifecycle: LifecycleCapability {
            supports_warm_pool: on,
            max_warm_pool_size: Some(PROFILE_MAX_WARM_POOL_SIZE),
            supports_named_tags: on,
            supports_prewarm_per_tenant: on,
            supports_pause_resume: off,
            supports_native_auto_stop: off,
            max_lease_lifetime_seconds: None,
            dormancy: DormancyCapability::NONE,
        },
        performance: PerformanceProfile {
            cold_start_p50_ms: COLD_START_P50_MS,
            cold_restore_p50_ms: COLD_RESTORE_P50_MS,
            max_concurrent_sandboxes: Some(PROFILE_MAX_CONCURRENT_SANDBOXES),
        },
        resources: ResourceCapability {
            vcpu_millis: ResourceRange::new(MIN_VCPU_MILLIS, MAX_VCPU_MILLIS).ok(),
            ..ResourceCapability::default()
        },
        placement: PlacementCapability::default(),
        ingress: IngressCapability::default(),
    }
}

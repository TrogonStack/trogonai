use trogon_sandbox_contract::{
    CapabilitySupport, DormancyCapability, EgressCapability, EgressEnforcementLevel, IngressCapability,
    IsolationCapability, IsolationTier, LifecycleCapability, PerformanceProfile, PlacementCapability,
    ProviderCapabilityProfile, ProviderKind, ResourceCapability, ResourceLimitMechanism, ResourceRange, SandboxSpec,
    SnapshotCapability, VolumeCapability, VolumeMode,
};

use crate::ModalConfig;
use crate::constants::{COLD_START_P50_MS, MAX_LEASE_LIFETIME_SECONDS, MIN_VCPU_MILLIS};

/// What a worker with `config` offers.
///
/// Snapshots are provider-native filesystem images, not entries in the Trogon snapshot store, so only the
/// native verbs are claimed. FUSE mounts are not offered: a mount lives on the worker host and a Modal sandbox
/// runs in Modal's cloud. S3 volumes are staged through worker-local scratch instead.
pub(crate) fn for_config(config: &ModalConfig) -> ProviderCapabilityProfile {
    let off = CapabilitySupport::Unsupported;
    let on = CapabilitySupport::Supported;
    ProviderCapabilityProfile {
        provider: ProviderKind::Modal,
        snapshot: SnapshotCapability {
            can_snapshot: off,
            can_checkpoint_running: on,
            can_restore: on,
            supported_artifact_types: Vec::new(),
            layered_deltas: off,
            max_snapshot_size_bytes: None,
            supports_native_snapshots: on,
            supports_in_place_rollback: off,
        },
        volume: VolumeCapability {
            supported_modes: vec![VolumeMode::ReadOnly, VolumeMode::ReadWrite],
            supports_s3_backend: on,
            supports_fuse_mounts: off,
        },
        isolation: IsolationCapability {
            default_tier: IsolationTier::UserSpaceKernel,
            supported_tiers: vec![IsolationTier::UserSpaceKernel],
            egress_enforcement: EgressEnforcementLevel::BestEffort,
            supports_privileged: off,
            supports_sandbox_users: off,
            supports_interactive_stdin: off,
            supports_named_processes: off,
            supports_process_readiness: off,
            supports_graceful_stop: off,
            supports_command_restart: off,
            supports_command_cancellation: off,
            supports_command_launch_cancellation: off,
            supports_port_preview: on,
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
            supports_terminal: off,
            resource_limits: ResourceLimitMechanism::None,
            egress: EgressCapability::DENY_ONLY,
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
            supports_warm_pool: off,
            max_warm_pool_size: None,
            supports_named_tags: on,
            supports_prewarm_per_tenant: off,
            supports_pause_resume: off,
            supports_native_auto_stop: off,
            max_lease_lifetime_seconds: Some(MAX_LEASE_LIFETIME_SECONDS),
            dormancy: DormancyCapability::NONE,
        },
        performance: PerformanceProfile {
            cold_start_p50_ms: COLD_START_P50_MS,
            cold_restore_p50_ms: 0,
            max_concurrent_sandboxes: None,
        },
        resources: ResourceCapability {
            vcpu_millis: ResourceRange::new(MIN_VCPU_MILLIS, config.max_vcpus.max(1).saturating_mul(1000)).ok(),
            ..ResourceCapability::default()
        },
        placement: PlacementCapability {
            regions: config.regions.clone(),
            ..PlacementCapability::default()
        },
        ingress: IngressCapability {
            max_exposed_ports: u16::try_from(SandboxSpec::MAX_EXPOSED_PORTS).ok(),
        },
    }
}

use std::sync::LazyLock;

use trogon_sandbox_contract::CapabilitySupport::{Supported, Unsupported};
use trogon_sandbox_contract::{
    DormancyCapability, EgressCapability, EgressEnforcementLevel, IsolationCapability, IsolationTier,
    LifecycleCapability, PerformanceProfile, PlacementCapability, ProviderCapabilityProfile, ProviderKind,
    ResourceLimitMechanism, SnapshotCapability, SuspendLevel, VolumeCapability,
};

/// What this worker actually does: run scripted commands over a file map. It
/// enforces nothing, snapshots nothing and mounts nothing, so everything beyond
/// the base verbs is declared unsupported.
pub static PROFILE: LazyLock<ProviderCapabilityProfile> = LazyLock::new(|| ProviderCapabilityProfile {
    provider: ProviderKind::Mock,
    snapshot: SnapshotCapability {
        can_snapshot: Unsupported,
        can_checkpoint_running: Unsupported,
        can_restore: Unsupported,
        supported_artifact_types: Vec::new(),
        layered_deltas: Unsupported,
        supports_native_snapshots: Unsupported,
        supports_in_place_rollback: Unsupported,
        max_snapshot_size_bytes: None,
    },
    volume: VolumeCapability {
        supported_modes: Vec::new(),
        supports_s3_backend: Unsupported,
        supports_fuse_mounts: Unsupported,
    },
    isolation: IsolationCapability {
        default_tier: IsolationTier::Container,
        supported_tiers: vec![IsolationTier::Container],
        egress_enforcement: EgressEnforcementLevel::None,
        supports_privileged: Unsupported,
        supports_sandbox_users: Unsupported,
        supports_interactive_stdin: Unsupported,
        supports_named_processes: Supported,
        supports_process_readiness: Supported,
        supports_graceful_stop: Supported,
        supports_command_restart: Supported,
        supports_command_cancellation: Unsupported,
        supports_command_launch_cancellation: Unsupported,
        supports_port_preview: Unsupported,
        supports_expose_after_create: Unsupported,
        supports_evidence_streaming: Unsupported,
        supports_output_streaming: Unsupported,
        supports_domain_allowlist: Unsupported,
        supports_tls_intercept: Unsupported,
        supports_secret_substitution: Unsupported,
        supports_secret_bindings: Unsupported,
        supports_masked_files: Unsupported,
        supports_fs_policy: Unsupported,
        supports_file_ops: Unsupported,
        supports_file_search: Unsupported,
        supports_file_archive: Unsupported,
        supports_output_redaction: Supported,
        supports_terminal: Supported,
        resource_limits: ResourceLimitMechanism::None,
        egress: EgressCapability::NONE,
        supports_hardening_overrides: Unsupported,
        supports_change_tracking: Supported,
        supports_file_watch: Supported,
        supports_persistent_runtime: Unsupported,
        supports_dir_transfer: Unsupported,
        supports_browser: Unsupported,
        supports_network_policy_update: Unsupported,
        supports_sandbox_groups: Unsupported,
    },
    lifecycle: LifecycleCapability {
        supports_warm_pool: Unsupported,
        max_warm_pool_size: None,
        supports_named_tags: Unsupported,
        supports_prewarm_per_tenant: Unsupported,
        supports_pause_resume: Supported,
        supports_native_auto_stop: Unsupported,
        max_lease_lifetime_seconds: None,
        // Pause freezes the lease in place; memory suspend does the same
        // with the budget and lease states of the suspend verbs.
        dormancy: DormancyCapability {
            freeze: Supported,
            suspend: Some(SuspendLevel::Memory),
        },
    },
    performance: PerformanceProfile {
        cold_start_p50_ms: 0,
        cold_restore_p50_ms: 0,
        max_concurrent_sandboxes: None,
    },
    resources: Default::default(),
    // The worker accepts a `label` native option and can report a native
    // handle, so tests can drive the passthrough path without a vendor.
    placement: PlacementCapability {
        regions: Vec::new(),
        supports_native_passthrough: Supported,
        ..Default::default()
    },
    ingress: Default::default(),
});

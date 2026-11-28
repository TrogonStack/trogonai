use std::sync::LazyLock;

use trogon_sandbox_contract::{
    CapabilitySupport::{Supported, Unsupported},
    DormancyCapability, EgressCapability, EgressEnforcementLevel, IsolationCapability, IsolationTier,
    LifecycleCapability, PerformanceProfile, ProviderCapabilityProfile, ProviderKind, ResourceLimitMechanism,
    SnapshotCapability, VolumeCapability, VolumeMode,
};

/// What this worker can and cannot do, as advertised in its heartbeat.
///
/// The tier is `MicroVm`: the boundary is a hardware-virtualized guest (KVM, MSHV or WHP through Hyperlight) with its
/// own kernel (Unikraft), as with Firecracker; unlike `WasmMicroVm` there is no Wasm sandbox inside it, the guest runs
/// native Linux binaries.
pub static PROFILE: LazyLock<ProviderCapabilityProfile> = LazyLock::new(|| ProviderCapabilityProfile {
    provider: ProviderKind::Hluk,
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
        supported_modes: vec![VolumeMode::ReadOnly],
        supports_s3_backend: Unsupported,
        supports_fuse_mounts: Unsupported,
    },
    isolation: IsolationCapability {
        default_tier: IsolationTier::MicroVm,
        supported_tiers: vec![IsolationTier::MicroVm],
        // hluk owns every guest socket on the host side, so a guest without --net connects nowhere. Kept at best
        // effort until the conformance denied-connect check proves it on this worker.
        egress_enforcement: EgressEnforcementLevel::BestEffort,
        supports_privileged: Unsupported,
        supports_sandbox_users: Unsupported,
        supports_interactive_stdin: Unsupported,
        supports_named_processes: Unsupported,
        supports_process_readiness: Unsupported,
        supports_graceful_stop: Unsupported,
        supports_command_restart: Unsupported,
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
        supports_file_ops: Supported,
        supports_file_search: Supported,
        supports_file_archive: Supported,
        supports_output_redaction: Unsupported,
        supports_terminal: Unsupported,
        // Guest memory is the size each runtime image is tested with; the lease's limits are not applied.
        resource_limits: ResourceLimitMechanism::None,
        egress: EgressCapability::DENY_ONLY,
        supports_hardening_overrides: Unsupported,
        supports_change_tracking: Unsupported,
        supports_file_watch: Unsupported,
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
        supports_pause_resume: Unsupported,
        supports_native_auto_stop: Unsupported,
        max_lease_lifetime_seconds: None,
        dormancy: DormancyCapability::NONE,
    },
    performance: PerformanceProfile {
        cold_start_p50_ms: 20,
        cold_restore_p50_ms: 17,
        max_concurrent_sandboxes: None,
    },
    resources: Default::default(),
    placement: Default::default(),
    ingress: Default::default(),
});

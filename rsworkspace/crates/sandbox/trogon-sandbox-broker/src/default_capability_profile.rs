//! The capability profile of the mock provider, advertised by the in-memory
//! worker registry and used as the fixture profile in tests.

use trogon_sandbox_contract::{
    ArtifactType, CapabilitySupport, DormancyCapability, EgressCapability, EgressEnforcementLevel, IsolationCapability,
    IsolationTier, LifecycleCapability, PerformanceProfile, ProviderCapabilityProfile, ProviderKind,
    ResourceCapability, ResourceLimitMechanism, ResourceRange, SnapshotCapability, VolumeCapability, VolumeMode,
};

/// A profile that supports snapshots, volumes, FUSE, warm pools, tags and
/// every egress feature, and none of the command and port surfaces.
pub fn default_capability_profile() -> ProviderCapabilityProfile {
    let yes = CapabilitySupport::Supported;
    let no = CapabilitySupport::Unsupported;
    ProviderCapabilityProfile {
        provider: ProviderKind::Mock,
        snapshot: SnapshotCapability {
            can_snapshot: yes,
            can_checkpoint_running: yes,
            can_restore: yes,
            supported_artifact_types: vec![ArtifactType::SandboxRootfsV1, ArtifactType::HyperlightSnapshotV1],
            layered_deltas: no,
            supports_native_snapshots: no,
            supports_in_place_rollback: no,
            max_snapshot_size_bytes: None,
        },
        volume: VolumeCapability {
            supported_modes: vec![VolumeMode::ReadOnly, VolumeMode::ReadWrite, VolumeMode::CopyOnWrite],
            supports_s3_backend: yes,
            supports_fuse_mounts: yes,
        },
        isolation: IsolationCapability {
            default_tier: IsolationTier::Container,
            supported_tiers: vec![
                IsolationTier::Container,
                IsolationTier::UserSpaceKernel,
                IsolationTier::MicroVm,
                IsolationTier::FullVm,
                IsolationTier::WasmMicroVm,
            ],
            egress_enforcement: EgressEnforcementLevel::Mandatory,
            supports_privileged: yes,
            supports_sandbox_users: yes,
            supports_interactive_stdin: no,
            supports_named_processes: no,
            supports_process_readiness: no,
            supports_graceful_stop: no,
            supports_command_restart: no,
            supports_command_cancellation: no,
            supports_command_launch_cancellation: no,
            supports_port_preview: no,
            supports_expose_after_create: no,
            supports_evidence_streaming: no,
            supports_output_streaming: no,
            supports_domain_allowlist: yes,
            supports_tls_intercept: yes,
            supports_secret_substitution: yes,
            supports_secret_bindings: yes,
            supports_masked_files: yes,
            supports_fs_policy: yes,
            supports_file_ops: yes,
            supports_file_search: yes,
            supports_file_archive: yes,
            supports_output_redaction: yes,
            supports_terminal: no,
            resource_limits: ResourceLimitMechanism::None,
            egress: EgressCapability::PROXIED,
            supports_hardening_overrides: no,
            supports_change_tracking: no,
            supports_file_watch: no,
            supports_persistent_runtime: no,
            supports_dir_transfer: no,
            supports_browser: no,
            supports_network_policy_update: no,
            supports_sandbox_groups: no,
        },
        lifecycle: LifecycleCapability {
            supports_warm_pool: yes,
            max_warm_pool_size: Some(8),
            supports_named_tags: yes,
            supports_prewarm_per_tenant: yes,
            supports_pause_resume: yes,
            supports_native_auto_stop: no,
            max_lease_lifetime_seconds: None,
            dormancy: DormancyCapability::FREEZE,
        },
        performance: PerformanceProfile {
            cold_start_p50_ms: 10,
            cold_restore_p50_ms: 10,
            max_concurrent_sandboxes: Some(32),
        },
        resources: ResourceCapability {
            vcpu_millis: ResourceRange::new(100, 64_000).ok(),
            disk_mib: ResourceRange::new(64, 1_048_576).ok(),
            reports_usage_gauges: no,
        },
        placement: Default::default(),
        ingress: Default::default(),
    }
}

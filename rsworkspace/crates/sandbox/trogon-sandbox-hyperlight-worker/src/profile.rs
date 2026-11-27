use trogon_sandbox_contract::{
    ArtifactType, CapabilitySupport, DormancyCapability, EgressCapability, EgressEnforcementLevel, IngressCapability,
    IsolationCapability, IsolationTier, LifecycleCapability, PerformanceProfile, ProviderCapabilityProfile,
    ProviderKind, ResourceCapability, ResourceLimitMechanism, SnapshotCapability, VolumeCapability, VolumeMode,
};

/// What the Hyperlight worker advertises: guests restore from and publish to Hyperlight
/// snapshots, keep a warm pool of `max_warm_sandboxes` runtimes and serve up to
/// `max_concurrent_leases` leases at once.
pub fn capability_profile(max_warm_sandboxes: u32, max_concurrent_leases: u32) -> ProviderCapabilityProfile {
    let yes = CapabilitySupport::Supported;
    let no = CapabilitySupport::Unsupported;
    ProviderCapabilityProfile {
        provider: ProviderKind::Hyperlight,
        snapshot: SnapshotCapability {
            can_snapshot: yes,
            can_checkpoint_running: yes,
            can_restore: yes,
            supported_artifact_types: vec![ArtifactType::HyperlightSnapshotV1],
            layered_deltas: no,
            max_snapshot_size_bytes: None,
            supports_native_snapshots: no,
            supports_in_place_rollback: no,
        },
        volume: VolumeCapability {
            supported_modes: vec![VolumeMode::ReadOnly, VolumeMode::ReadWrite],
            supports_s3_backend: no,
            supports_fuse_mounts: no,
        },
        isolation: IsolationCapability {
            default_tier: IsolationTier::WasmMicroVm,
            supported_tiers: vec![IsolationTier::WasmMicroVm],
            // Best effort, not mandatory: the worker serves deny-all egress only and nothing
            // here proves that a guest connect is refused.
            egress_enforcement: EgressEnforcementLevel::BestEffort,
            supports_privileged: no,
            supports_sandbox_users: no,
            supports_interactive_stdin: no,
            supports_named_processes: no,
            supports_process_readiness: no,
            supports_graceful_stop: no,
            supports_command_restart: no,
            supports_command_cancellation: no,
            supports_command_launch_cancellation: no,
            supports_port_preview: no,
            supports_expose_after_create: no,
            supports_evidence_streaming: yes,
            supports_output_streaming: no,
            supports_domain_allowlist: no,
            supports_tls_intercept: no,
            supports_secret_substitution: no,
            supports_secret_bindings: no,
            supports_masked_files: no,
            supports_fs_policy: no,
            supports_file_ops: no,
            supports_file_search: no,
            supports_file_archive: no,
            supports_output_redaction: no,
            supports_terminal: no,
            resource_limits: ResourceLimitMechanism::None,
            egress: EgressCapability::DENY_ONLY,
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
            max_warm_pool_size: Some(max_warm_sandboxes),
            supports_named_tags: yes,
            supports_prewarm_per_tenant: yes,
            supports_pause_resume: no,
            supports_native_auto_stop: no,
            max_lease_lifetime_seconds: None,
            dormancy: DormancyCapability::NONE,
        },
        performance: PerformanceProfile {
            cold_start_p50_ms: 50,
            cold_restore_p50_ms: 100,
            max_concurrent_sandboxes: Some(max_concurrent_leases),
        },
        resources: ResourceCapability::default(),
        placement: Default::default(),
        ingress: IngressCapability::default(),
    }
}

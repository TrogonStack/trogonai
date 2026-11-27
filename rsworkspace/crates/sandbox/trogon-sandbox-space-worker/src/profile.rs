use crate::MaxLeases;
use crate::constants::MAX_VCPU;
use trogon_sandbox_contract::{
    CapabilitySupport, DormancyCapability, EgressCapability, EgressEnforcementLevel, IngressCapability,
    IsolationCapability, IsolationTier, LifecycleCapability, PerformanceProfile, ProviderCapabilityProfile,
    ProviderKind, ResourceCapability, ResourceLimitMechanism, ResourceRange, SnapshotCapability, SuspendLevel,
    VolumeCapability,
};

/// What this worker does today, not what SPACE could do. Each unsupported entry is a
/// feature the worker does not wire, even where SPACE offers a native form of it;
/// advertising it would route work here that the worker would then have to refuse.
pub fn capability_profile(max_leases: MaxLeases) -> ProviderCapabilityProfile {
    let no = CapabilitySupport::Unsupported;
    ProviderCapabilityProfile {
        provider: ProviderKind::Space,
        snapshot: SnapshotCapability {
            can_snapshot: no,
            can_checkpoint_running: no,
            can_restore: no,
            supported_artifact_types: Vec::new(),
            layered_deltas: no,
            max_snapshot_size_bytes: None,
            supports_native_snapshots: no,
            supports_in_place_rollback: no,
        },
        volume: VolumeCapability {
            supported_modes: Vec::new(),
            supports_s3_backend: no,
            supports_fuse_mounts: no,
        },
        isolation: IsolationCapability {
            default_tier: IsolationTier::MicroVm,
            supported_tiers: vec![IsolationTier::MicroVm],
            // The worker does not translate a restrictive network policy into a SPACE
            // one, so it must not claim to enforce any.
            egress_enforcement: EgressEnforcementLevel::None,
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
            supports_evidence_streaming: no,
            // Output is buffered and capped by the worker; nothing is forwarded while
            // the command runs.
            supports_output_streaming: no,
            supports_domain_allowlist: no,
            supports_tls_intercept: no,
            supports_secret_substitution: no,
            supports_secret_bindings: no,
            supports_masked_files: no,
            supports_fs_policy: no,
            // File reads and writes go through guest commands, so the richer file
            // operations are not available.
            supports_file_ops: no,
            supports_file_search: no,
            supports_file_archive: no,
            supports_output_redaction: no,
            supports_terminal: no,
            resource_limits: ResourceLimitMechanism::None,
            egress: EgressCapability::NONE,
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
            supports_warm_pool: no,
            max_warm_pool_size: None,
            supports_named_tags: no,
            supports_prewarm_per_tenant: no,
            // Pause keeps VM memory resident (a freeze); suspend saves VM memory and the
            // root filesystem and releases the VM. Both return to running with wake.
            supports_pause_resume: CapabilitySupport::Supported,
            supports_native_auto_stop: no,
            max_lease_lifetime_seconds: None,
            dormancy: DormancyCapability {
                freeze: CapabilitySupport::Supported,
                suspend: Some(SuspendLevel::Memory),
            },
        },
        performance: PerformanceProfile {
            cold_start_p50_ms: 0,
            cold_restore_p50_ms: 0,
            max_concurrent_sandboxes: Some(max_leases.get()),
        },
        resources: ResourceCapability {
            vcpu_millis: ResourceRange::new(1000, MAX_VCPU * 1000).ok(),
            ..ResourceCapability::default()
        },
        placement: Default::default(),
        ingress: IngressCapability {
            max_exposed_ports: Some(0),
        },
    }
}

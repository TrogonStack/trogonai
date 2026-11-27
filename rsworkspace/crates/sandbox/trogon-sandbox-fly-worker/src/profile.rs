use trogon_sandbox_contract::{
    CapabilitySupport, DormancyCapability, EgressCapability, EgressEnforcementLevel, IngressCapability,
    IsolationCapability, IsolationTier, LifecycleCapability, PerformanceProfile, PlacementCapability,
    ProviderCapabilityProfile, ProviderKind, ResourceCapability, ResourceLimitMechanism, ResourceRange, SandboxSpec,
    SnapshotCapability, VolumeCapability, VolumeMode,
};

use crate::FlyConfig;
use crate::constants::{MAX_VCPU_MILLIS, MIN_VCPU_MILLIS};

/// What a worker with `config` offers. Native snapshots exist only with a workspace volume, and the volume
/// modes only with an operator volume.
pub(crate) fn for_config(config: &FlyConfig) -> ProviderCapabilityProfile {
    let off = CapabilitySupport::Unsupported;
    let workspace = CapabilitySupport::from(config.workspace_volume.is_some());
    let pooled = CapabilitySupport::from(config.warm_pool_size > 0);
    let volume_modes = if config.volume.is_some() {
        vec![VolumeMode::ReadWrite]
    } else {
        Vec::new()
    };
    ProviderCapabilityProfile {
        provider: ProviderKind::Fly,
        snapshot: SnapshotCapability {
            can_snapshot: off,
            can_checkpoint_running: off,
            can_restore: workspace,
            supported_artifact_types: Vec::new(),
            layered_deltas: off,
            max_snapshot_size_bytes: None,
            supports_native_snapshots: workspace,
            supports_in_place_rollback: off,
        },
        volume: VolumeCapability {
            supported_modes: volume_modes,
            supports_s3_backend: off,
            supports_fuse_mounts: off,
        },
        isolation: IsolationCapability {
            default_tier: IsolationTier::MicroVm,
            supported_tiers: vec![IsolationTier::MicroVm],
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
            supports_port_preview: CapabilitySupport::Supported,
            supports_expose_after_create: off,
            supports_evidence_streaming: off,
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
            supports_warm_pool: pooled,
            max_warm_pool_size: u32::try_from(config.warm_pool_size).ok().filter(|size| *size > 0),
            supports_named_tags: off,
            supports_prewarm_per_tenant: off,
            supports_pause_resume: CapabilitySupport::Supported,
            supports_native_auto_stop: off,
            max_lease_lifetime_seconds: None,
            dormancy: DormancyCapability::SUSPEND_MEMORY,
        },
        performance: PerformanceProfile {
            cold_start_p50_ms: 0,
            cold_restore_p50_ms: 0,
            max_concurrent_sandboxes: u32::try_from(config.max_leases).ok(),
        },
        resources: ResourceCapability {
            vcpu_millis: ResourceRange::new(MIN_VCPU_MILLIS, MAX_VCPU_MILLIS).ok(),
            ..ResourceCapability::default()
        },
        placement: PlacementCapability {
            regions: vec![config.region.as_str().to_owned()],
            ..PlacementCapability::default()
        },
        ingress: IngressCapability {
            max_exposed_ports: u16::try_from(SandboxSpec::MAX_EXPOSED_PORTS).ok(),
        },
    }
}

use std::sync::LazyLock;

use crate::constants::{DEFAULT_MAX_VCPUS, MILLIS_PER_VCPU};

use trogon_sandbox_contract::{
    CapabilitySupport, DormancyCapability, EgressCapability, EgressEnforcementLevel, IngressCapability,
    IsolationCapability, IsolationTier, LifecycleCapability, PerformanceProfile, PlacementCapability,
    ProviderCapabilityProfile, ProviderKind, ResourceCapability, ResourceLimitMechanism, ResourceRange, SandboxSpec,
    SnapshotCapability, VolumeCapability, VolumeMode,
};

/// The sizes this worker claims: whole vCPUs from one up to `max_vcpus`, advertised in millis. Memory follows the
/// vCPU count and Vercel offers no disk option.
pub fn resources(max_vcpus: u32) -> ResourceCapability {
    ResourceCapability {
        vcpu_millis: ResourceRange::new(MILLIS_PER_VCPU, max_vcpus.max(1).saturating_mul(MILLIS_PER_VCPU)).ok(),
        disk_mib: None,
        reports_usage_gauges: CapabilitySupport::Unsupported,
    }
}

/// [`PROFILE`] with `resources` swapped in.
pub fn profile_with_resources(resources: ResourceCapability) -> ProviderCapabilityProfile {
    ProviderCapabilityProfile {
        resources,
        ..PROFILE.clone()
    }
}

/// What this worker can honestly do on Vercel Sandbox.
///
/// Declared true only where the worker implements the feature against the Vercel API. Native snapshots are served
/// and restored through the sandbox source. OCI export stays off: Vercel snapshots are Vercel owned images, not the
/// OCI artifacts the snapshot flags describe. Pause and resume are declared with stop and start semantics: a stopped
/// persistent sandbox keeps its files but not its processes.
pub static PROFILE: LazyLock<ProviderCapabilityProfile> = LazyLock::new(|| {
    let off = CapabilitySupport::Unsupported;
    let on = CapabilitySupport::Supported;
    ProviderCapabilityProfile {
        provider: ProviderKind::Vercel,
        snapshot: SnapshotCapability {
            can_snapshot: off,
            can_checkpoint_running: off,
            can_restore: off,
            supported_artifact_types: Vec::new(),
            layered_deltas: off,
            max_snapshot_size_bytes: None,
            supports_native_snapshots: on,
            supports_in_place_rollback: off,
        },
        volume: VolumeCapability {
            supported_modes: vec![VolumeMode::ReadWrite],
            supports_s3_backend: off,
            supports_fuse_mounts: off,
        },
        isolation: IsolationCapability {
            default_tier: IsolationTier::MicroVm,
            supported_tiers: vec![IsolationTier::MicroVm],
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
            supports_domain_allowlist: on,
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
            egress: EgressCapability {
                deny_all: on,
                host_allowlist: on,
                cidr_allowlist: on,
                proxy_mediated: off,
            },
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
            supports_pause_resume: on,
            supports_native_auto_stop: off,
            max_lease_lifetime_seconds: Some(86_400),
            dormancy: DormancyCapability::SUSPEND_DISK,
        },
        performance: PerformanceProfile {
            cold_start_p50_ms: 1000,
            cold_restore_p50_ms: 2000,
            max_concurrent_sandboxes: None,
        },
        resources: resources(DEFAULT_MAX_VCPUS),
        placement: PlacementCapability {
            regions: Vec::new(),
            supports_native_passthrough: on,
            ..PlacementCapability::default()
        },
        ingress: IngressCapability {
            max_exposed_ports: u16::try_from(SandboxSpec::MAX_EXPOSED_PORTS).ok(),
        },
    }
});

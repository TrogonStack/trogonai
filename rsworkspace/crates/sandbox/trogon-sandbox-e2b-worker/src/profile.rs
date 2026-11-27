use std::sync::LazyLock;

use trogon_sandbox_contract::{
    CapabilitySupport::{Supported, Unsupported},
    DormancyCapability, EgressCapability, EgressEnforcementLevel, IsolationCapability, IsolationTier,
    LifecycleCapability, PerformanceProfile, PlacementCapability, ProviderCapabilityProfile, ProviderKind,
    ResourceLimitMechanism, SnapshotCapability, VolumeCapability, VolumeMode,
};

#[cfg(test)]
mod tests;

/// What this worker can honestly do on E2B.
pub static PROFILE: LazyLock<ProviderCapabilityProfile> = LazyLock::new(|| ProviderCapabilityProfile {
    provider: ProviderKind::E2b,
    snapshot: SnapshotCapability {
        // E2B memory snapshots restore from provider templates rather than OCI artifacts.
        can_snapshot: Unsupported,
        can_checkpoint_running: Supported,
        can_restore: Supported,
        supported_artifact_types: vec![],
        layered_deltas: Unsupported,
        supports_native_snapshots: Supported,
        supports_in_place_rollback: Unsupported,
        max_snapshot_size_bytes: None,
    },
    volume: VolumeCapability {
        supported_modes: vec![VolumeMode::ReadWrite],
        supports_s3_backend: Unsupported,
        // The FUSE pipeline mounts on the worker host and bind-mounts into the guest.
        // E2B offers no way to hand a microVM a host path.
        supports_fuse_mounts: Unsupported,
    },
    isolation: IsolationCapability {
        default_tier: IsolationTier::MicroVm,
        supported_tiers: vec![IsolationTier::MicroVm],
        egress_enforcement: EgressEnforcementLevel::BestEffort,
        supports_privileged: Unsupported,
        supports_sandbox_users: Unsupported,
        supports_interactive_stdin: Unsupported,
        supports_named_processes: Unsupported,
        supports_process_readiness: Unsupported,
        supports_graceful_stop: Unsupported,
        supports_command_restart: Unsupported,
        supports_command_cancellation: Supported,
        supports_command_launch_cancellation: Unsupported,
        supports_port_preview: Supported,
        // E2B serves every port of a running sandbox at <port>-<id>.<domain>.
        supports_expose_after_create: Supported,
        supports_evidence_streaming: Supported,
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
        supports_terminal: Supported,
        resource_limits: ResourceLimitMechanism::None,
        egress: EgressCapability {
            deny_all: Supported,
            host_allowlist: Supported,
            cidr_allowlist: Supported,
            proxy_mediated: Unsupported,
        },
        supports_hardening_overrides: Unsupported,
        supports_change_tracking: Supported,
        supports_file_watch: Supported,
        supports_persistent_runtime: Unsupported,
        supports_dir_transfer: Supported,
        supports_browser: Unsupported,
        supports_network_policy_update: Unsupported,
        supports_sandbox_groups: Unsupported,
    },
    lifecycle: LifecycleCapability {
        supports_warm_pool: Unsupported,
        max_warm_pool_size: None,
        supports_named_tags: Supported,
        supports_prewarm_per_tenant: Unsupported,
        // Memory-preserving pause: processes and files survive.
        supports_pause_resume: Supported,
        supports_native_auto_stop: Unsupported,
        max_lease_lifetime_seconds: Some(86_400),
        dormancy: DormancyCapability::SUSPEND_MEMORY,
    },
    performance: PerformanceProfile {
        cold_start_p50_ms: 1000,
        cold_restore_p50_ms: 2000,
        max_concurrent_sandboxes: None,
    },
    // E2B fixes cpu and memory in the template, so a per-lease size cannot be enforced
    // and a request that names one is refused at admission.
    resources: Default::default(),
    placement: PlacementCapability {
        regions: Vec::new(),
        supports_native_passthrough: Supported,
        ..Default::default()
    },
    ingress: Default::default(),
});

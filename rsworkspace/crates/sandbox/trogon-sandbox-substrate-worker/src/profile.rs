use std::sync::LazyLock;

use trogon_sandbox_contract::{
    CapabilitySupport::{Supported, Unsupported},
    DormancyCapability, EgressCapability, EgressEnforcementLevel, IngressCapability, IsolationCapability,
    IsolationTier, LifecycleCapability, PerformanceProfile, ProviderCapabilityProfile, ProviderKind,
    ResourceLimitMechanism, SnapshotCapability, VolumeCapability, VolumeMode,
};

/// Honest capability profile for Agent Substrate.
///
/// Native snapshots support retained actors, but this adapter does not export signed OCI
/// artifacts. Advertising portable snapshots would route requests here that the worker
/// cannot fulfill. Native snapshot metadata and object references do not establish that
/// portable artifact contract.
///
/// The same reasoning applies to `can_restore`: an actor can be created from a *substrate*
/// snapshot ref, but not from the `OciArtifactRef` the rest of the stack means by "restore".
pub static PROFILE: LazyLock<ProviderCapabilityProfile> = LazyLock::new(|| ProviderCapabilityProfile {
    provider: ProviderKind::Substrate,
    snapshot: SnapshotCapability {
        can_snapshot: Unsupported,
        can_checkpoint_running: Unsupported,
        can_restore: Unsupported,
        supported_artifact_types: vec![],
        layered_deltas: Unsupported,
        supports_native_snapshots: Unsupported,
        supports_in_place_rollback: Unsupported,
        max_snapshot_size_bytes: None,
    },
    // This adapter provisions an actor-local durable directory. It does not translate
    // the sandbox drive or remote-mount contracts into upstream volume resources.
    volume: VolumeCapability {
        supported_modes: vec![VolumeMode::ReadWrite],
        supports_s3_backend: Unsupported,
        supports_fuse_mounts: Unsupported,
    },
    isolation: IsolationCapability {
        default_tier: IsolationTier::UserSpaceKernel,
        supported_tiers: vec![IsolationTier::UserSpaceKernel, IsolationTier::MicroVm],
        // Upstream exposes egress policy RPCs, but this adapter does not drive them.
        // Advertising enforcement would admit policies the worker cannot fulfill.
        egress_enforcement: EgressEnforcementLevel::None,
        supports_privileged: Unsupported,
        // The in-actor agent runs `useradd` itself and drops privileges before spawning
        // a command, so `run_as_user` is enforced by the kernel rather than honoured on
        // trust. An image without `useradd` answers `MissingDependency`, which is a
        // per-image fact rather than a provider capability.
        supports_sandbox_users: Supported,
        supports_interactive_stdin: Supported,
        supports_named_processes: Unsupported,
        supports_process_readiness: Unsupported,
        supports_graceful_stop: Unsupported,
        supports_command_restart: Unsupported,
        supports_command_cancellation: Unsupported,
        supports_command_launch_cancellation: Unsupported,
        supports_port_preview: Supported,
        supports_expose_after_create: Unsupported,
        supports_evidence_streaming: Supported,
        supports_output_streaming: Supported,
        supports_domain_allowlist: Unsupported,
        supports_tls_intercept: Unsupported,
        supports_secret_substitution: Unsupported,
        supports_secret_bindings: Unsupported,
        supports_masked_files: Unsupported,
        supports_fs_policy: Unsupported,
        supports_file_ops: Unsupported,
        supports_file_search: Unsupported,
        supports_file_archive: Unsupported,
        supports_output_redaction: Unsupported,
        supports_terminal: Unsupported,
        resource_limits: ResourceLimitMechanism::None,
        egress: EgressCapability::NONE,
        supports_hardening_overrides: Unsupported,
        supports_change_tracking: Unsupported,
        supports_file_watch: Unsupported,
        supports_persistent_runtime: Unsupported,
        supports_dir_transfer: Unsupported,
        supports_browser: Unsupported,
        supports_network_policy_update: Unsupported,
        supports_sandbox_groups: Supported,
    },
    lifecycle: LifecycleCapability {
        supports_warm_pool: Unsupported,
        max_warm_pool_size: None,
        supports_named_tags: Supported,
        // A named lease's actor stays suspended between leases and resumes from its own
        // golden state, which is per-tenant prewarming by construction.
        supports_prewarm_per_tenant: Supported,
        supports_pause_resume: Unsupported,
        supports_native_auto_stop: Unsupported,
        max_lease_lifetime_seconds: None,
        dormancy: DormancyCapability::NONE,
    },
    performance: PerformanceProfile {
        // [!] Estimates, not measurements. Substrate's resume path restores from a
        // golden snapshot rather than booting, which is why the restore figure is below
        // the cold-start one, but neither number has been measured against a real
        // cluster yet. Replace both with observed p50s before these are used for
        // routing decisions that matter.
        cold_start_p50_ms: 1500,
        cold_restore_p50_ms: 500,
        // Capacity lives in the `WorkerPool`s, which this worker does not size and
        // cannot query for headroom. The per-worker cap comes from configuration
        // instead, and is reported through the heartbeat rather than the profile.
        max_concurrent_sandboxes: None,
    },
    resources: Default::default(),
    placement: Default::default(),
    ingress: IngressCapability {
        max_exposed_ports: Some(1),
    },
});

#[cfg(test)]
mod tests;

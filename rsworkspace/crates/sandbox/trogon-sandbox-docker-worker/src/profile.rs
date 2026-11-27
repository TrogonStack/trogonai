use std::sync::LazyLock;

use trogon_sandbox_contract::{
    ArtifactType, CapabilitySupport, DormancyCapability, EgressEnforcementLevel, IsolationCapability, IsolationTier,
    LifecycleCapability, PerformanceProfile, PlacementCapability, ProviderCapabilityProfile, ProviderKind,
    ResourceCapability, ResourceRange, SnapshotCapability, VolumeCapability, VolumeMode,
};

mod constants;

use constants::{MIN_DISK_MIB, MIN_VCPU_MILLIS};

/// The ranges this worker claims. Each maximum is clamped up to its minimum, so a range is always valid.
/// CPU is always enforceable (NanoCPUs). A disk
/// quota rides on `StorageOpt size`, which only some storage drivers accept
/// (overlay2 on xfs with `pquota`, zfs, btrfs), so it is advertised only when
/// the operator names a ceiling.
pub fn resources(max_vcpus: u32, max_disk_mib: Option<u32>) -> ResourceCapability {
    ResourceCapability {
        vcpu_millis: ResourceRange::new(MIN_VCPU_MILLIS, max_vcpus.saturating_mul(1000).max(MIN_VCPU_MILLIS)).ok(),
        disk_mib: max_disk_mib.and_then(|max| ResourceRange::new(MIN_DISK_MIB, max.max(MIN_DISK_MIB)).ok()),
        reports_usage_gauges: CapabilitySupport::Supported,
    }
}

/// [`PROFILE`] with `resources` swapped in.
pub fn profile_with_resources(resources: ResourceCapability) -> ProviderCapabilityProfile {
    ProviderCapabilityProfile {
        resources,
        ..PROFILE.clone()
    }
}

pub static PROFILE: LazyLock<ProviderCapabilityProfile> = LazyLock::new(|| ProviderCapabilityProfile {
    provider: ProviderKind::LocalDocker,
    snapshot: SnapshotCapability {
        can_snapshot: CapabilitySupport::Supported,
        can_checkpoint_running: CapabilitySupport::Supported,
        can_restore: CapabilitySupport::Supported,
        supported_artifact_types: vec![ArtifactType::SandboxRootfsV1],
        layered_deltas: CapabilitySupport::Unsupported,
        supports_native_snapshots: CapabilitySupport::Unsupported,
        supports_in_place_rollback: CapabilitySupport::Supported,
        max_snapshot_size_bytes: None,
    },
    volume: VolumeCapability {
        supported_modes: vec![VolumeMode::ReadOnly, VolumeMode::ReadWrite, VolumeMode::CopyOnWrite],
        supports_s3_backend: CapabilitySupport::Supported,
        supports_fuse_mounts: CapabilitySupport::Supported,
    },
    isolation: IsolationCapability {
        default_tier: IsolationTier::Container,
        supported_tiers: vec![IsolationTier::Container],
        egress_enforcement: EgressEnforcementLevel::BestEffort,
        supports_privileged: CapabilitySupport::Unsupported,
        supports_sandbox_users: CapabilitySupport::Supported,
        supports_interactive_stdin: CapabilitySupport::Supported,
        supports_named_processes: CapabilitySupport::Supported,
        supports_process_readiness: CapabilitySupport::Supported,
        supports_graceful_stop: CapabilitySupport::Supported,
        supports_command_restart: CapabilitySupport::Supported,
        supports_command_cancellation: CapabilitySupport::Unsupported,
        supports_command_launch_cancellation: CapabilitySupport::Unsupported,
        supports_port_preview: CapabilitySupport::Supported,
        supports_expose_after_create: CapabilitySupport::Supported,
        supports_evidence_streaming: CapabilitySupport::Supported,
        supports_output_streaming: CapabilitySupport::Supported,
        supports_domain_allowlist: CapabilitySupport::Supported,
        supports_tls_intercept: CapabilitySupport::Supported,
        supports_secret_substitution: CapabilitySupport::Supported,
        supports_secret_bindings: CapabilitySupport::Unsupported,
        supports_masked_files: CapabilitySupport::Unsupported,
        supports_fs_policy: CapabilitySupport::Unsupported,
        supports_file_ops: CapabilitySupport::Supported,
        supports_file_search: CapabilitySupport::Supported,
        supports_file_archive: CapabilitySupport::Supported,
        supports_output_redaction: CapabilitySupport::Supported,
        supports_terminal: CapabilitySupport::Supported,
        resource_limits: trogon_sandbox_contract::ResourceLimitMechanism::None,
        egress: trogon_sandbox_contract::EgressCapability::PROXIED,
        supports_hardening_overrides: CapabilitySupport::Supported,
        supports_change_tracking: CapabilitySupport::Supported,
        supports_file_watch: CapabilitySupport::Supported,
        supports_persistent_runtime: CapabilitySupport::Unsupported,
        supports_dir_transfer: CapabilitySupport::Supported,
        supports_browser: CapabilitySupport::Unsupported,
        supports_network_policy_update: CapabilitySupport::Supported,
        supports_sandbox_groups: CapabilitySupport::Supported,
    },
    lifecycle: LifecycleCapability {
        supports_warm_pool: CapabilitySupport::Supported,
        max_warm_pool_size: Some(16),
        supports_named_tags: CapabilitySupport::Supported,
        supports_prewarm_per_tenant: CapabilitySupport::Supported,
        supports_pause_resume: CapabilitySupport::Supported,
        supports_native_auto_stop: CapabilitySupport::Unsupported,
        max_lease_lifetime_seconds: None,
        dormancy: DormancyCapability::FREEZE,
    },
    performance: PerformanceProfile {
        cold_start_p50_ms: 500,
        cold_restore_p50_ms: 5000,
        max_concurrent_sandboxes: Some(4),
    },
    resources: ResourceCapability {
        reports_usage_gauges: CapabilitySupport::Supported,
        ..ResourceCapability::default()
    },
    placement: PlacementCapability {
        // The container engine is the worker's own host: no region choice.
        regions: Vec::new(),
        supports_native_passthrough: CapabilitySupport::Supported,
        ..Default::default()
    },
    ingress: Default::default(),
});

#[cfg(test)]
mod tests;

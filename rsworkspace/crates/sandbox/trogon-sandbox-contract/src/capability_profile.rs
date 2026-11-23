use serde::{Deserialize, Serialize};

use crate::{
    ArtifactType, CapabilitySupport, DormancyCapability, EgressCapability, EgressEnforcementLevel, IngressCapability,
    IsolationTier, PlacementCapability, ProviderCapabilityProfile, ProviderKind, ResourceCapability,
    ResourceLimitMechanism, VolumeMode,
};

/// Tenant-facing, flattened view of a [`ProviderCapabilityProfile`].
///
/// It keeps only fields a tenant can act on, such as choosing a provider or isolation tier for a
/// lease request.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct CapabilityProfile {
    pub provider: ProviderKind,
    pub default_isolation_tier: IsolationTier,
    pub supported_isolation_tiers: Vec<IsolationTier>,
    pub egress_enforcement: EgressEnforcementLevel,
    pub can_snapshot: CapabilitySupport,
    pub can_checkpoint_running: CapabilitySupport,
    pub can_restore: CapabilitySupport,
    pub supported_artifact_types: Vec<ArtifactType>,
    pub supports_s3_volumes: CapabilitySupport,
    pub supports_fuse_mounts: CapabilitySupport,
    #[serde(default)]
    pub supports_native_snapshots: CapabilitySupport,
    #[serde(default)]
    pub supports_interactive_stdin: CapabilitySupport,
    #[serde(default)]
    pub supports_named_processes: CapabilitySupport,
    #[serde(default)]
    pub supports_process_readiness: CapabilitySupport,
    #[serde(default)]
    pub supports_graceful_stop: CapabilitySupport,
    #[serde(default)]
    pub supports_command_restart: CapabilitySupport,
    #[serde(default)]
    pub supports_command_cancellation: CapabilitySupport,
    #[serde(default)]
    pub supports_command_launch_cancellation: CapabilitySupport,
    pub supports_port_preview: CapabilitySupport,
    #[serde(default)]
    pub supports_expose_after_create: CapabilitySupport,
    #[serde(default)]
    pub supports_evidence_streaming: CapabilitySupport,
    #[serde(default)]
    pub supports_domain_allowlist: CapabilitySupport,
    #[serde(default)]
    pub supports_tls_intercept: CapabilitySupport,
    #[serde(default)]
    pub supports_secret_substitution: CapabilitySupport,
    #[serde(default)]
    pub supports_secret_bindings: CapabilitySupport,
    #[serde(default)]
    pub supports_masked_files: CapabilitySupport,
    #[serde(default)]
    pub supports_fs_policy: CapabilitySupport,
    #[serde(default)]
    pub supports_file_ops: CapabilitySupport,
    #[serde(default)]
    pub supports_file_search: CapabilitySupport,
    #[serde(default)]
    pub supports_file_archive: CapabilitySupport,
    #[serde(default)]
    pub supports_output_redaction: CapabilitySupport,
    #[serde(default)]
    pub supports_terminal: CapabilitySupport,
    #[serde(default)]
    pub resource_limits: ResourceLimitMechanism,
    #[serde(default)]
    pub supports_change_tracking: CapabilitySupport,
    #[serde(default)]
    pub supports_file_watch: CapabilitySupport,
    #[serde(default)]
    pub supports_dir_transfer: CapabilitySupport,
    pub supports_browser: CapabilitySupport,
    pub supported_volume_modes: Vec<VolumeMode>,
    pub supports_warm_pool: CapabilitySupport,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_warm_pool_size: Option<u32>,
    pub supports_named_tags: CapabilitySupport,
    pub supports_prewarm_per_tenant: CapabilitySupport,
    #[serde(default)]
    pub supports_pause_resume: CapabilitySupport,
    #[serde(default)]
    pub dormancy: DormancyCapability,
    pub cold_start_p50_ms: u32,
    pub cold_restore_p50_ms: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_concurrent_sandboxes: Option<u32>,
    #[serde(default)]
    pub resources: ResourceCapability,
    #[serde(default)]
    pub egress: EgressCapability,
    #[serde(default)]
    pub supports_hardening_overrides: CapabilitySupport,
    #[serde(default)]
    pub placement: PlacementCapability,
    #[serde(default)]
    pub ingress: IngressCapability,
    #[serde(default)]
    pub supports_persistent_runtime: CapabilitySupport,
    #[serde(default)]
    pub supports_network_policy_update: CapabilitySupport,
    #[serde(default)]
    pub supports_sandbox_groups: CapabilitySupport,
    #[serde(default)]
    pub supports_native_auto_stop: CapabilitySupport,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_lease_lifetime_seconds: Option<u32>,
}

impl From<&ProviderCapabilityProfile> for CapabilityProfile {
    fn from(profile: &ProviderCapabilityProfile) -> Self {
        let snapshot = &profile.snapshot;
        let volume = &profile.volume;
        let isolation = &profile.isolation;
        let lifecycle = &profile.lifecycle;
        let performance = &profile.performance;
        Self {
            provider: profile.provider,
            default_isolation_tier: isolation.default_tier,
            supported_isolation_tiers: isolation.supported_tiers.clone(),
            egress_enforcement: isolation.egress_enforcement,
            can_snapshot: snapshot.can_snapshot,
            can_checkpoint_running: snapshot.can_checkpoint_running,
            can_restore: snapshot.can_restore,
            supported_artifact_types: snapshot.supported_artifact_types.clone(),
            supports_s3_volumes: volume.supports_s3_backend,
            supports_fuse_mounts: volume.supports_fuse_mounts,
            supports_native_snapshots: snapshot.supports_native_snapshots,
            supports_interactive_stdin: isolation.supports_interactive_stdin,
            supports_named_processes: isolation.supports_named_processes,
            supports_process_readiness: isolation.supports_process_readiness,
            supports_graceful_stop: isolation.supports_graceful_stop,
            supports_command_restart: isolation.supports_command_restart,
            supports_command_cancellation: isolation.supports_command_cancellation,
            supports_command_launch_cancellation: CapabilitySupport::from(
                isolation.supports_command_cancellation.is_supported()
                    && isolation.supports_command_launch_cancellation.is_supported(),
            ),
            supports_port_preview: isolation.supports_port_preview,
            supports_expose_after_create: isolation.supports_expose_after_create,
            supports_evidence_streaming: isolation.supports_evidence_streaming,
            supports_domain_allowlist: isolation.supports_domain_allowlist,
            supports_tls_intercept: isolation.supports_tls_intercept,
            supports_secret_substitution: isolation.supports_secret_substitution,
            supports_secret_bindings: isolation.supports_secret_bindings,
            supports_masked_files: isolation.supports_masked_files,
            supports_fs_policy: isolation.supports_fs_policy,
            supports_file_ops: isolation.supports_file_ops,
            supports_file_search: isolation.supports_file_search,
            supports_file_archive: isolation.supports_file_archive,
            supports_output_redaction: isolation.supports_output_redaction,
            supports_terminal: isolation.supports_terminal,
            resource_limits: isolation.resource_limits,
            supports_change_tracking: isolation.supports_change_tracking,
            supports_file_watch: isolation.supports_file_watch,
            supports_dir_transfer: isolation.supports_dir_transfer,
            supports_browser: isolation.supports_browser,
            supported_volume_modes: volume.supported_modes.clone(),
            supports_warm_pool: lifecycle.supports_warm_pool,
            max_warm_pool_size: lifecycle.max_warm_pool_size,
            supports_named_tags: lifecycle.supports_named_tags,
            supports_prewarm_per_tenant: lifecycle.supports_prewarm_per_tenant,
            supports_pause_resume: lifecycle.supports_pause_resume,
            dormancy: lifecycle.dormancy,
            cold_start_p50_ms: performance.cold_start_p50_ms,
            cold_restore_p50_ms: performance.cold_restore_p50_ms,
            max_concurrent_sandboxes: performance.max_concurrent_sandboxes,
            resources: profile.resources,
            egress: isolation.egress,
            supports_hardening_overrides: isolation.supports_hardening_overrides,
            placement: profile.placement.clone(),
            ingress: profile.ingress,
            supports_persistent_runtime: isolation.supports_persistent_runtime,
            supports_network_policy_update: isolation.supports_network_policy_update,
            supports_sandbox_groups: isolation.supports_sandbox_groups,
            supports_native_auto_stop: lifecycle.supports_native_auto_stop,
            max_lease_lifetime_seconds: lifecycle.max_lease_lifetime_seconds,
        }
    }
}

impl From<ProviderCapabilityProfile> for CapabilityProfile {
    fn from(profile: ProviderCapabilityProfile) -> Self {
        Self::from(&profile)
    }
}

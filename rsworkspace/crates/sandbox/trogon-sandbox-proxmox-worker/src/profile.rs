use trogon_sandbox_contract::{
    CapabilitySupport, DormancyCapability, EgressCapability, EgressEnforcementLevel, IngressCapability,
    IsolationCapability, IsolationTier, LifecycleCapability, PerformanceProfile, PlacementCapability,
    ProviderCapabilityProfile, ProviderKind, ResourceCapability, ResourceLimitMechanism, ResourceRange,
    SnapshotCapability, VolumeCapability,
};

use crate::ProxmoxConfig;
use crate::constants::{MAX_VCPU_MILLIS, MIN_VCPU_MILLIS};

/// What a worker with `config` offers. Without a proxy network a guest has no NIC at all and only `DenyAll`
/// is served; with one, leases may name a domain allowlist that the egress proxy enforces.
pub(crate) fn for_config(config: &ProxmoxConfig) -> ProviderCapabilityProfile {
    let off = CapabilitySupport::Unsupported;
    let proxied = config.network.is_some();
    ProviderCapabilityProfile {
        provider: ProviderKind::Proxmox,
        snapshot: SnapshotCapability {
            can_snapshot: off,
            can_checkpoint_running: off,
            can_restore: off,
            supported_artifact_types: Vec::new(),
            layered_deltas: off,
            max_snapshot_size_bytes: None,
            supports_native_snapshots: off,
            supports_in_place_rollback: off,
        },
        volume: VolumeCapability {
            supported_modes: Vec::new(),
            supports_s3_backend: off,
            supports_fuse_mounts: off,
        },
        isolation: IsolationCapability {
            default_tier: IsolationTier::FullVm,
            supported_tiers: vec![IsolationTier::MicroVm, IsolationTier::FullVm],
            // Guest NIC anti-spoofing is requested explicitly, not left to defaults: with a proxy network the
            // worker sets `firewall=1`, `macfilter=1`, `ipfilter=1` and a one-address `ipfilter-net0`, reads
            // them back and refuses the lease under `--antispoof require`. Without one the clone has no NIC at
            // all. What the pve-firewall service then applies to the running tap stays with Proxmox. Reported
            // as best effort rather than mandatory: nothing here proves a guest connect is refused.
            egress_enforcement: EgressEnforcementLevel::BestEffort,
            supports_privileged: CapabilitySupport::Supported,
            supports_sandbox_users: off,
            supports_interactive_stdin: off,
            supports_named_processes: off,
            supports_process_readiness: off,
            supports_graceful_stop: off,
            supports_command_restart: off,
            supports_command_cancellation: off,
            supports_command_launch_cancellation: off,
            supports_port_preview: off,
            supports_expose_after_create: off,
            supports_evidence_streaming: off,
            supports_output_streaming: off,
            supports_domain_allowlist: CapabilitySupport::from(proxied),
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
            egress: if proxied {
                EgressCapability::PROXIED
            } else {
                EgressCapability::DENY_ONLY
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
            supports_named_tags: off,
            supports_prewarm_per_tenant: off,
            supports_pause_resume: off,
            supports_native_auto_stop: off,
            max_lease_lifetime_seconds: None,
            dormancy: DormancyCapability::NONE,
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
        placement: PlacementCapability::default(),
        ingress: IngressCapability {
            max_exposed_ports: Some(0),
        },
    }
}

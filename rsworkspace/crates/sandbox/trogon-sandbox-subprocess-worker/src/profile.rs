#[cfg(target_os = "linux")]
use trogon_sandbox_contract::ResourceRange;
use trogon_sandbox_contract::{
    CapabilitySupport, DormancyCapability, EgressCapability, EgressEnforcementLevel, IngressCapability,
    IsolationCapability, IsolationTier, LifecycleCapability, PerformanceProfile, PlacementCapability,
    ProviderCapabilityProfile, ProviderKind, ResourceCapability, ResourceLimitMechanism, SnapshotCapability,
    VolumeCapability, VolumeMode,
};

#[cfg(target_os = "linux")]
use crate::isolation::limits::{LimitMechanism, ResourceEnforcer};

fn support(on: bool) -> CapabilitySupport {
    CapabilitySupport::from(on)
}

/// What the worker advertises for a backend that confines the payload.
pub(crate) fn base_profile() -> ProviderCapabilityProfile {
    let off = CapabilitySupport::Unsupported;
    let on = CapabilitySupport::Supported;
    let unix_files = support(cfg!(any(target_os = "linux", target_os = "macos")));
    let not_windows = support(!cfg!(windows));
    ProviderCapabilityProfile {
        provider: ProviderKind::Subprocess,
        snapshot: SnapshotCapability {
            can_snapshot: off,
            can_checkpoint_running: off,
            can_restore: off,
            supported_artifact_types: vec![],
            layered_deltas: off,
            supports_native_snapshots: off,
            supports_in_place_rollback: off,
            max_snapshot_size_bytes: None,
        },
        volume: VolumeCapability {
            supported_modes: vec![VolumeMode::ReadOnly, VolumeMode::ReadWrite, VolumeMode::CopyOnWrite],
            supports_s3_backend: off,
            supports_fuse_mounts: support(cfg!(target_os = "linux")),
        },
        isolation: IsolationCapability {
            default_tier: default_tier(),
            supported_tiers: supported_tiers(),
            egress_enforcement: EgressEnforcementLevel::Mandatory,
            supports_privileged: off,
            // A deliberate architectural limit, not a missing feature. Each lease runs in its
            // own per-process user namespace rather than under any host-visible identity, and
            // the worker itself runs unprivileged, so it cannot write the host account
            // databases. `useradd`, `groupadd`, `usermod` and `gpasswd` run inside the sandbox
            // would fail or change namespace-local state that vanishes with the process. The
            // worker answers the user and group verbs with a typed denial instead.
            supports_sandbox_users: off,
            supports_interactive_stdin: off,
            supports_named_processes: off,
            supports_process_readiness: off,
            supports_graceful_stop: off,
            supports_command_restart: off,
            supports_command_cancellation: off,
            supports_command_launch_cancellation: off,
            // A port is reached by entering the lease's network namespace, which only the
            // bubblewrap backend creates. A lease that shares the host network answers
            // `Unsupported` itself.
            supports_port_preview: support(cfg!(target_os = "linux")),
            supports_expose_after_create: support(cfg!(target_os = "linux")),
            supports_evidence_streaming: on,
            supports_output_streaming: off,
            supports_domain_allowlist: not_windows,
            supports_tls_intercept: not_windows,
            // The worker mints sentinels and hands the registry to the lease proxy, which swaps
            // them at the destinations each binding names. `Transform` rules share this flag:
            // the proxy fetches their credentials from the broker the worker is configured
            // with, and a lease that needs either without one is refused at launch.
            supports_secret_substitution: not_windows,
            supports_secret_bindings: not_windows,
            // Seatbelt has no mount namespace to bind a fake copy over a file.
            supports_masked_files: support(cfg!(target_os = "linux")),
            // Enforced by the mount plan of the bubblewrap backend; the passthrough backend
            // refuses a lease that carries a policy. The AppContainer backend sees only the
            // directories the worker grants it and cannot express a policy inside them.
            supports_fs_policy: not_windows,
            // Served from the host side of the workspace by the `file_op` handler of the Linux
            // and macOS workers; the Windows worker does not subscribe to per-lease subjects.
            supports_file_ops: unix_files,
            supports_file_search: unix_files,
            supports_file_archive: unix_files,
            // The worker serves no run_command verb: the lease's own process is the only output
            // it produces, so there is no command output to filter.
            supports_output_redaction: off,
            // Served by the `terminal` handler: each terminal is a pseudo-terminal launch
            // through the lease's own backend, which both the bubblewrap and the seatbelt
            // backends can attach. The Windows worker has no pseudo-terminal.
            supports_terminal: unix_files,
            resource_limits: resource_limit_mechanism(),
            egress: if cfg!(windows) {
                EgressCapability::DENY_ONLY
            } else {
                EgressCapability::PROXIED
            },
            supports_hardening_overrides: off,
            supports_change_tracking: unix_files,
            supports_file_watch: unix_files,
            supports_persistent_runtime: off,
            supports_dir_transfer: unix_files,
            supports_browser: off,
            supports_network_policy_update: off,
            supports_sandbox_groups: on,
        },
        lifecycle: LifecycleCapability {
            supports_warm_pool: off,
            max_warm_pool_size: None,
            supports_named_tags: on,
            supports_prewarm_per_tenant: off,
            supports_pause_resume: off,
            supports_native_auto_stop: off,
            max_lease_lifetime_seconds: None,
            dormancy: DormancyCapability::NONE,
        },
        performance: PerformanceProfile {
            cold_start_p50_ms: 100,
            cold_restore_p50_ms: 100,
            max_concurrent_sandboxes: Some(4),
        },
        resources: machine_resources(),
        placement: PlacementCapability::default(),
        ingress: IngressCapability::default(),
    }
}

/// What the worker advertises for a given isolation backend.
///
/// [`PROFILE`](crate::PROFILE) describes a backend that confines the payload. The backend
/// `none` spawns the payload as a plain host process: it has no network
/// namespace, no mount plan, no proxy to route through and nothing that hides a
/// secret, so claiming mandatory egress enforcement for it would let the broker
/// route `DenyAll` leases to a worker that serves them with open egress.
pub fn profile_for_backend(backend: crate::backend_kind::IsolationBackendKind) -> ProviderCapabilityProfile {
    let mut profile = base_profile();
    if matches!(backend, crate::backend_kind::IsolationBackendKind::None) {
        let off = CapabilitySupport::Unsupported;
        let isolation = &mut profile.isolation;
        isolation.egress_enforcement = EgressEnforcementLevel::None;
        isolation.egress = EgressCapability::NONE;
        isolation.supports_domain_allowlist = off;
        isolation.supports_tls_intercept = off;
        isolation.supports_secret_substitution = off;
        isolation.supports_secret_bindings = off;
        isolation.supports_masked_files = off;
        isolation.supports_fs_policy = off;
        isolation.supports_port_preview = off;
        isolation.supports_expose_after_create = off;
    }
    profile
}

/// What a Windows worker advertises once the network fence is installed: the
/// lease proxy serves a domain allowlist and terminates TLS with a CA the
/// sandbox account trusts, and the fence helper's `exec` subcommand enforces write
/// roots and read and write denies.
#[cfg(windows)]
pub(crate) fn fenced_profile() -> ProviderCapabilityProfile {
    let on = CapabilitySupport::Supported;
    let mut profile = base_profile();
    profile.isolation.supports_domain_allowlist = on;
    profile.isolation.egress = EgressCapability::PROXIED;
    profile.isolation.supports_tls_intercept = on;
    profile.isolation.supports_fs_policy = on;
    profile
}

#[cfg(target_os = "linux")]
fn default_tier() -> IsolationTier {
    IsolationTier::Bwrap
}

#[cfg(target_os = "linux")]
fn supported_tiers() -> Vec<IsolationTier> {
    vec![IsolationTier::Bwrap, IsolationTier::Container]
}

/// Seatbelt and an AppContainer are neither a user-namespace sandbox nor a bare container, and no
/// tier names it. Claiming `Bwrap` would promise a network and mount
/// namespace it does not have.
#[cfg(not(target_os = "linux"))]
fn default_tier() -> IsolationTier {
    IsolationTier::Container
}

#[cfg(not(target_os = "linux"))]
fn supported_tiers() -> Vec<IsolationTier> {
    vec![IsolationTier::Container]
}

/// Which Landlock level backs the filesystem policy as a second layer behind
/// the mounts, for the kernel the worker runs on.
#[cfg(target_os = "linux")]
pub fn fs_second_layer() -> crate::isolation::landlock::LandlockSupport {
    static SUPPORT: std::sync::LazyLock<crate::isolation::landlock::LandlockSupport> =
        std::sync::LazyLock::new(crate::isolation::landlock::LandlockSupport::detect);
    *SUPPORT
}

/// A vCPU share is a cgroup `cpu.max` quota, so it is advertised only when the
/// cpu controller is delegated. No disk quota exists for a host directory
/// workspace, so disk is never advertised.
#[cfg(target_os = "linux")]
fn machine_resources() -> ResourceCapability {
    let host_vcpus = std::thread::available_parallelism().map_or(1, |n| u32::try_from(n.get()).unwrap_or(u32::MAX));
    ResourceCapability {
        vcpu_millis: if ResourceEnforcer::detect().enforces_cpu_quota() {
            ResourceRange::new(100, host_vcpus.saturating_mul(1000)).ok()
        } else {
            None
        },
        disk_mib: None,
        reports_usage_gauges: CapabilitySupport::Unsupported,
    }
}

#[cfg(not(target_os = "linux"))]
fn machine_resources() -> ResourceCapability {
    ResourceCapability::default()
}

#[cfg(target_os = "linux")]
fn resource_limit_mechanism() -> ResourceLimitMechanism {
    match ResourceEnforcer::detect().mechanism() {
        LimitMechanism::CgroupV2 => ResourceLimitMechanism::CgroupV2,
        LimitMechanism::Rlimit => ResourceLimitMechanism::Rlimit,
    }
}

#[cfg(windows)]
fn resource_limit_mechanism() -> ResourceLimitMechanism {
    ResourceLimitMechanism::JobObject
}

#[cfg(not(any(target_os = "linux", windows)))]
fn resource_limit_mechanism() -> ResourceLimitMechanism {
    ResourceLimitMechanism::None
}

#[cfg(test)]
mod tests;

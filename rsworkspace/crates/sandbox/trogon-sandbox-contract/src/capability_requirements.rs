use serde::{Deserialize, Serialize};

use crate::{CapabilityNeed, EgressEnforcementLevel, EnvDigest, IsolationTier, SuspendLevel};

/// What a lease asks of a worker, one `needs_*` flag or limit per capability.
///
/// Every `needs_*` flag serializes as a JSON boolean and defaults to `false` when omitted. The
/// optional limits are omitted from JSON when absent.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize, Default)]
pub struct CapabilityRequirements {
    #[serde(default)]
    pub needs_snapshot: CapabilityNeed,
    #[serde(default)]
    pub needs_checkpoint_running: CapabilityNeed,
    #[serde(default)]
    pub needs_restore: CapabilityNeed,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_isolation_tier: Option<IsolationTier>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_cold_restore_p50_ms: Option<u32>,
    #[serde(default)]
    pub needs_warm_pool: CapabilityNeed,
    #[serde(default)]
    pub needs_named_tags: CapabilityNeed,
    #[serde(default)]
    pub needs_fuse_mounts: CapabilityNeed,
    #[serde(default)]
    pub needs_privileged: CapabilityNeed,
    #[serde(default)]
    pub needs_sandbox_users: CapabilityNeed,
    #[serde(default)]
    pub needs_interactive_stdin: CapabilityNeed,
    #[serde(default)]
    pub needs_named_processes: CapabilityNeed,
    /// The caller reads command output the worker streamed (`read_logs`).
    #[serde(default)]
    pub needs_output_streaming: CapabilityNeed,
    #[serde(default)]
    pub needs_process_readiness: CapabilityNeed,
    /// The caller intends to stop a command gracefully. Checked against the
    /// worker's graceful stop support.
    #[serde(default)]
    pub needs_graceful_stop: CapabilityNeed,
    /// The caller intends to restart a command. Checked against the
    /// worker's command restart support.
    #[serde(default)]
    pub needs_command_restart: CapabilityNeed,
    /// The spec declares ports to expose.
    #[serde(default)]
    pub needs_port_preview: CapabilityNeed,
    /// How many ports the spec declares. The worker's
    /// ingress `max_exposed_ports` must cover it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exposed_port_count: Option<u16>,
    #[serde(default)]
    pub needs_domain_allowlist: CapabilityNeed,
    /// Minimum [`EgressEnforcementLevel`] a worker must declare. Set for any
    /// network policy that denies all traffic or lists user-defined rules, so a worker that
    /// ignores the policy is never handed a lease that depends on it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_egress_enforcement: Option<EgressEnforcementLevel>,
    #[serde(default)]
    pub needs_tls_intercept: CapabilityNeed,
    #[serde(default)]
    pub needs_secret_substitution: CapabilityNeed,
    #[serde(default)]
    pub needs_secret_bindings: CapabilityNeed,
    #[serde(default)]
    pub needs_masked_files: CapabilityNeed,
    #[serde(default)]
    pub needs_fs_policy: CapabilityNeed,
    /// The caller intends to pause the lease. Checked against the
    /// worker's pause and resume support.
    #[serde(default)]
    pub needs_pause_resume: CapabilityNeed,
    /// The caller intends to suspend the lease to this level. Checked against the
    /// worker's suspend support.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub needs_suspend: Option<SuspendLevel>,
    #[serde(default)]
    pub needs_file_ops: CapabilityNeed,
    #[serde(default)]
    pub needs_file_search: CapabilityNeed,
    #[serde(default)]
    pub needs_file_archive: CapabilityNeed,
    /// A command registers values to redact. Checked against the
    /// worker's output redaction support.
    #[serde(default)]
    pub needs_output_redaction: CapabilityNeed,
    /// The caller wants live resource gauges for a lease. Checked against the
    /// worker's resource gauge support.
    #[serde(default)]
    pub needs_usage_gauges: CapabilityNeed,
    /// The caller intends to open a terminal. Checked against the
    /// worker's terminal support.
    #[serde(default)]
    pub needs_terminal: CapabilityNeed,
    /// The caller intends to replace the network policy of a running lease.
    /// Checked against the worker's network policy update support.
    #[serde(default)]
    pub needs_network_policy_update: CapabilityNeed,
    /// The caller intends to create a sandbox group or change membership.
    /// Checked against the worker's sandbox group support.
    #[serde(default)]
    pub needs_sandbox_groups: CapabilityNeed,
    /// Requested virtual CPU, in thousandths of a core. The worker's advertised range
    /// must contain it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vcpu_millis: Option<u32>,
    /// Requested disk size, in MiB. The worker's advertised range must
    /// contain it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disk_mib: Option<u32>,
    /// A user-defined network policy made only of IP ranges. Met by a worker
    /// that declares CIDR allowlist support, or by any worker with domain allowlist support,
    /// whose proxy always handles ranges.
    #[serde(default)]
    pub needs_cidr_allowlist: CapabilityNeed,
    /// The spec carries container hardening overrides.
    #[serde(default)]
    pub needs_hardening_overrides: CapabilityNeed,
    /// The lease carries `native` vendor options. Only a worker that declares
    /// native passthrough support in its placement capability may receive it.
    #[serde(default)]
    pub needs_native_passthrough: CapabilityNeed,
    /// The environment digest a restore must run under. Only a worker whose
    /// placement `env_digest` equals it may receive the lease.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub env_digest: Option<EnvDigest>,
    /// The one region this placement attempt requires. It is the lease's
    /// preferred region first, then each fallback region in order.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub region: Option<String>,
    /// The caller wants checkpoint and changes on a lease's files. Checked against
    /// the worker's change tracking support.
    #[serde(default)]
    pub needs_change_tracking: CapabilityNeed,
    /// The caller wants to watch a path in a lease. Checked against the
    /// worker's file watch support.
    #[serde(default)]
    pub needs_file_watch: CapabilityNeed,
    /// The spec asks for a persistent runtime.
    #[serde(default)]
    pub needs_persistent_runtime: CapabilityNeed,
    /// The caller wants a verified directory transfer. Checked against the worker's directory transfer support.
    #[serde(default)]
    pub needs_dir_transfer: CapabilityNeed,
    /// The caller wants a headless browser inside the sandbox.
    #[serde(default)]
    pub needs_browser: CapabilityNeed,
    /// How long the lease must be able to live, in seconds. A worker whose
    /// lifecycle `max_lease_lifetime_seconds` is lower cannot take it.
    /// [`Self::NO_TTL_LIFETIME`] stands for a `no_ttl` lease.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lease_lifetime_seconds: Option<u32>,
}

impl CapabilityRequirements {
    /// The lifetime a `no_ttl` lease needs: more than any capped worker offers.
    pub const NO_TTL_LIFETIME: u32 = u32::MAX;
}

use serde::{Deserialize, Serialize};

use crate::{EgressCapability, EgressEnforcementLevel, IsolationTier, ResourceLimitMechanism};

/// Execution, file, network, and process features a worker serves inside its isolation boundary.
///
/// Every flag defaults to `false`, so a profile from a worker that predates a flag claims nothing
/// beyond what its older fields say.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct IsolationCapability {
    pub default_tier: IsolationTier,
    pub supported_tiers: Vec<IsolationTier>,
    pub egress_enforcement: EgressEnforcementLevel,
    #[serde(default)]
    pub supports_privileged: bool,
    #[serde(default)]
    pub supports_sandbox_users: bool,
    #[serde(default)]
    pub supports_interactive_stdin: bool,
    #[serde(default)]
    pub supports_named_processes: bool,
    #[serde(default)]
    pub supports_process_readiness: bool,
    /// Stops a command with a termination signal, a grace period, then a kill.
    #[serde(default)]
    pub supports_graceful_stop: bool,
    /// Re-runs a stored command under a new execution id.
    #[serde(default)]
    pub supports_command_restart: bool,
    #[serde(default)]
    pub supports_command_cancellation: bool,
    #[serde(default)]
    pub supports_command_launch_cancellation: bool,
    /// Resolves a port declared at create time to a reachable domain.
    #[serde(default)]
    pub supports_port_preview: bool,
    /// Also resolves ports that were not declared at create time.
    #[serde(default)]
    pub supports_expose_after_create: bool,
    #[serde(default)]
    pub supports_evidence_streaming: bool,
    #[serde(default)]
    pub supports_output_streaming: bool,
    #[serde(default)]
    pub supports_domain_allowlist: bool,
    #[serde(default)]
    pub supports_tls_intercept: bool,
    #[serde(default)]
    pub supports_secret_substitution: bool,
    #[serde(default)]
    pub supports_secret_bindings: bool,
    #[serde(default)]
    pub supports_masked_files: bool,
    #[serde(default)]
    pub supports_fs_policy: bool,
    /// Lists, stats, reads, writes, edits, removes, creates, and renames files.
    #[serde(default)]
    pub supports_file_ops: bool,
    #[serde(default)]
    pub supports_file_search: bool,
    #[serde(default)]
    pub supports_file_archive: bool,
    /// Replaces requested literal values in command output before it leaves the worker.
    #[serde(default)]
    pub supports_output_redaction: bool,
    #[serde(default)]
    pub supports_terminal: bool,
    #[serde(default)]
    pub resource_limits: ResourceLimitMechanism,
    #[serde(default)]
    pub egress: EgressCapability,
    #[serde(default)]
    pub supports_hardening_overrides: bool,
    #[serde(default)]
    pub supports_change_tracking: bool,
    #[serde(default)]
    pub supports_file_watch: bool,
    /// Keeps one runtime alive across commands on a lease, so later commands see earlier state.
    #[serde(default)]
    pub supports_persistent_runtime: bool,
    #[serde(default)]
    pub supports_dir_transfer: bool,
    #[serde(default)]
    pub supports_browser: bool,
    /// Applies a network policy change to a running lease in place.
    #[serde(default)]
    pub supports_network_policy_update: bool,
    #[serde(default)]
    pub supports_sandbox_groups: bool,
}

use serde::{Deserialize, Serialize};

use crate::{CapabilitySupport, EgressCapability, EgressEnforcementLevel, IsolationTier, ResourceLimitMechanism};

/// Execution, file, network, and process features a worker serves inside its isolation boundary.
///
/// Every flag defaults to `Unsupported`, so a profile from a worker that predates a flag claims nothing
/// beyond what its older fields say.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct IsolationCapability {
    pub default_tier: IsolationTier,
    pub supported_tiers: Vec<IsolationTier>,
    pub egress_enforcement: EgressEnforcementLevel,
    #[serde(default)]
    pub supports_privileged: CapabilitySupport,
    #[serde(default)]
    pub supports_sandbox_users: CapabilitySupport,
    #[serde(default)]
    pub supports_interactive_stdin: CapabilitySupport,
    #[serde(default)]
    pub supports_named_processes: CapabilitySupport,
    #[serde(default)]
    pub supports_process_readiness: CapabilitySupport,
    /// Stops a command with a termination signal, a grace period, then a kill.
    #[serde(default)]
    pub supports_graceful_stop: CapabilitySupport,
    /// Re-runs a stored command under a new execution id.
    #[serde(default)]
    pub supports_command_restart: CapabilitySupport,
    #[serde(default)]
    pub supports_command_cancellation: CapabilitySupport,
    #[serde(default)]
    pub supports_command_launch_cancellation: CapabilitySupport,
    /// Resolves a port declared at create time to a reachable domain.
    #[serde(default)]
    pub supports_port_preview: CapabilitySupport,
    /// Also resolves ports that were not declared at create time.
    #[serde(default)]
    pub supports_expose_after_create: CapabilitySupport,
    #[serde(default)]
    pub supports_evidence_streaming: CapabilitySupport,
    #[serde(default)]
    pub supports_output_streaming: CapabilitySupport,
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
    /// Lists, stats, reads, writes, edits, removes, creates, and renames files.
    #[serde(default)]
    pub supports_file_ops: CapabilitySupport,
    #[serde(default)]
    pub supports_file_search: CapabilitySupport,
    #[serde(default)]
    pub supports_file_archive: CapabilitySupport,
    /// Replaces requested literal values in command output before it leaves the worker.
    #[serde(default)]
    pub supports_output_redaction: CapabilitySupport,
    #[serde(default)]
    pub supports_terminal: CapabilitySupport,
    #[serde(default)]
    pub resource_limits: ResourceLimitMechanism,
    #[serde(default)]
    pub egress: EgressCapability,
    #[serde(default)]
    pub supports_hardening_overrides: CapabilitySupport,
    #[serde(default)]
    pub supports_change_tracking: CapabilitySupport,
    #[serde(default)]
    pub supports_file_watch: CapabilitySupport,
    /// Keeps one runtime alive across commands on a lease, so later commands see earlier state.
    #[serde(default)]
    pub supports_persistent_runtime: CapabilitySupport,
    #[serde(default)]
    pub supports_dir_transfer: CapabilitySupport,
    #[serde(default)]
    pub supports_browser: CapabilitySupport,
    /// Applies a network policy change to a running lease in place.
    #[serde(default)]
    pub supports_network_policy_update: CapabilitySupport,
    #[serde(default)]
    pub supports_sandbox_groups: CapabilitySupport,
}

//! The words a refusal carries: operator text for a deny-list entry, or the standard reasons.

use trogon_sandbox_contract::{DenyReason, Host, NetworkOptions, NetworkPolicy, OpaqueTunnelPolicy};
use trogon_sandbox_protocol::{StandardDenyReason, deny_list_reason};

/// The standard words for `reason`.
pub(crate) fn standard_reason(reason: StandardDenyReason) -> Option<DenyReason> {
    reason.reason().ok()
}

/// The words explaining why `policy` denies `host:port`, if a deny-list entry names it.
pub(crate) fn deny_explanation(
    options: &NetworkOptions,
    policy: &NetworkPolicy,
    host: &Host,
    port: u16,
) -> Option<DenyReason> {
    deny_list_reason(options, policy, host, port).ok().flatten()
}

/// What `policy` says of tunnels that do not start TLS.
pub(crate) fn opaque_tunnels(policy: &NetworkPolicy) -> OpaqueTunnelPolicy {
    match policy {
        NetworkPolicy::AllowAll | NetworkPolicy::DenyAll => OpaqueTunnelPolicy::Allow,
        NetworkPolicy::UserDefined { opaque_tunnels, .. } => *opaque_tunnels,
    }
}

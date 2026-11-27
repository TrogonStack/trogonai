use trogon_sandbox_contract::{LeaseError, NetworkPolicy};
use trogon_sandbox_providers::policy_needs_egress_proxy;

/// Refuses a network policy this worker cannot enforce, with a typed error.
///
/// Rules decided by hostname, denied domains and request rules are enforced by
/// the egress proxy sidecar, and the sidecar and the pod's `NetworkPolicy` share
/// one network namespace. The `NetworkPolicy` built from such a policy has no
/// rule that lets the proxy reach an upstream (a domain-only `UserDefined`
/// policy yields an empty egress list), so on a CNI that enforces it the
/// sandbox can reach nothing, and on one that does not the proxy is the only
/// control. Neither is the allowlist the lease asked for, and opening the
/// `NetworkPolicy` to let the proxy through would also open the non-TCP traffic
/// the proxy never sees. The worker therefore serves only `AllowAll`, `DenyAll`
/// and policies made of IP ranges.
#[expect(
    clippy::result_large_err,
    reason = "LeaseError is the contract's wire error; boxing it here would only be unboxed by every caller"
)]
pub(crate) fn ensure_policy_enforceable(policy: Option<&NetworkPolicy>) -> Result<(), LeaseError> {
    match policy {
        Some(policy) if policy_needs_egress_proxy(policy) => Err(LeaseError::InvalidNetworkPolicy {
            reason: "the k8s worker enforces network policy with a Kubernetes NetworkPolicy, which cannot match hostnames; use DenyAll or IP ranges".to_string(),
        }),
        _ => Ok(()),
    }
}

/// The policy a lease runs under: none means the worker's default, deny all.
pub(crate) fn effective_policy(policy: Option<&NetworkPolicy>) -> NetworkPolicy {
    policy.cloned().unwrap_or(NetworkPolicy::DenyAll)
}

use trogon_sandbox_contract::{DomainRuleAction, NetworkPolicy};

use crate::EgressPolicy;

/// Whether the egress sidecar will terminate TLS for this policy, so the workload
/// must be told to trust the sidecar's CA.
///
/// A policy with no `Transform`/`Forward` rule never makes the sidecar terminate TLS:
/// the CA file would stay empty and pointing a client at an empty CA bundle breaks
/// every TLS connection, not just the ones the policy means to intercept.
pub(crate) fn requires_tls_intercept(policy: &NetworkPolicy) -> bool {
    match policy {
        NetworkPolicy::AllowAll | NetworkPolicy::DenyAll => false,
        NetworkPolicy::UserDefined { rules, .. } => rules.iter().any(|rule| {
            matches!(
                rule.action,
                DomainRuleAction::Transform(_) | DomainRuleAction::Forward(_)
            )
        }),
    }
}

/// Docker network mode for a workload. `none` has no interface at all, so it also blocks DNS.
pub(crate) fn network_mode(network_policy: Option<&NetworkPolicy>, egress_policy: &EgressPolicy) -> &'static str {
    match network_policy {
        Some(NetworkPolicy::DenyAll) => "none",
        Some(NetworkPolicy::UserDefined { .. } | NetworkPolicy::AllowAll) => "bridge",
        None => match egress_policy {
            EgressPolicy::DenyAll => "none",
            EgressPolicy::AllowHosts(_) => "bridge",
        },
    }
}

#[cfg(test)]
mod tests;

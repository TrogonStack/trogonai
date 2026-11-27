use trogon_sandbox_contract::{DomainRuleAction, NetworkPolicy};

/// Whether the policy has a rule that terminates TLS for the connections it
/// matches, so the workload must trust the per-command CA. A pass-through
/// rule never terminates TLS, and a CA file that stays empty would break
/// every TLS connection the workload makes.
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

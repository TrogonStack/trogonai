use trogon_sandbox_contract::{DomainRuleAction, NetworkPolicy};

/// What a network policy needs from the lease proxy beyond filtering.
pub trait NetworkPolicyNeeds {
    /// Whether a rule fetches credentials and substitutes them into requests.
    fn requires_secret_substitution(&self) -> bool;

    /// Whether a rule makes the proxy terminate TLS for some connections, so the payload must trust the
    /// per-lease CA. Rules that only pass traffic through never do.
    fn requires_tls_intercept(&self) -> bool;
}

impl NetworkPolicyNeeds for NetworkPolicy {
    fn requires_secret_substitution(&self) -> bool {
        match self {
            NetworkPolicy::AllowAll | NetworkPolicy::DenyAll => false,
            NetworkPolicy::UserDefined { rules, .. } => rules
                .iter()
                .any(|rule| matches!(rule.action, DomainRuleAction::Transform(_))),
        }
    }

    fn requires_tls_intercept(&self) -> bool {
        match self {
            NetworkPolicy::AllowAll | NetworkPolicy::DenyAll => false,
            NetworkPolicy::UserDefined { rules, .. } => rules.iter().any(|rule| {
                matches!(
                    rule.action,
                    DomainRuleAction::Transform(_) | DomainRuleAction::Forward(_)
                )
            }),
        }
    }
}

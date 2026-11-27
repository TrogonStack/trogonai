use trogon_sandbox_contract::{DomainRuleAction, LeaseError, NetworkPolicy};

use super::constants::DENY_EVERYTHING;
use super::{E2bNetwork, NetworkRules};

fn invalid(reason: impl Into<String>) -> LeaseError {
    LeaseError::InvalidNetworkPolicy { reason: reason.into() }
}

/// `None` means the account default, which is unrestricted egress, the same as
/// [`NetworkPolicy::AllowAll`].
pub fn translate_network_policy(policy: Option<&NetworkPolicy>) -> Result<E2bNetwork, LeaseError> {
    let (rules, allow_ranges, deny_ranges, denied_domains, opaque_tunnels, request_rules) = match policy {
        None | Some(NetworkPolicy::AllowAll) => return Ok(E2bNetwork::default()),
        Some(NetworkPolicy::DenyAll) => return Ok(E2bNetwork::denied()),
        Some(NetworkPolicy::UserDefined {
            rules,
            allow_ranges,
            deny_ranges,
            denied_domains,
            opaque_tunnels,
            request_rules,
        }) => (
            rules,
            allow_ranges,
            deny_ranges,
            denied_domains,
            opaque_tunnels,
            request_rules,
        ),
    };

    if !denied_domains.is_empty() {
        return Err(invalid("E2B cannot express denied_domains"));
    }
    if !request_rules.is_empty() {
        return Err(invalid("E2B cannot express request_rules"));
    }
    if !opaque_tunnels.is_allow() {
        return Err(invalid("E2B cannot refuse opaque tunnels"));
    }

    let mut allow: Vec<String> = Vec::new();
    for rule in rules {
        if !matches!(rule.action, DomainRuleAction::PassThrough) {
            return Err(invalid(format!(
                "E2B cannot transform or forward traffic for {}",
                rule.domain
            )));
        }
        if rule.domain.port().is_some() {
            return Err(invalid(format!("E2B cannot restrict by port: {}", rule.domain)));
        }
        allow.push(rule.domain.to_string());
    }
    for range in allow_ranges {
        if range.port().is_some() {
            return Err(invalid(format!("E2B cannot restrict by port: {range}")));
        }
        allow.push(range.cidr().as_str().to_owned());
    }
    allow.sort();
    allow.dedup();

    if allow.is_empty() {
        return Ok(E2bNetwork::denied());
    }
    let mut deny_out = vec![DENY_EVERYTHING.to_owned()];
    deny_out.extend(deny_ranges.iter().map(|range| range.as_str().to_owned()));
    deny_out.dedup();
    Ok(E2bNetwork {
        allow_internet_access: None,
        network: Some(NetworkRules {
            allow_out: allow,
            deny_out,
        }),
    })
}

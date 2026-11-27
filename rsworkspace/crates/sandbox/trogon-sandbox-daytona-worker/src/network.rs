//! Translation of the wire [`NetworkPolicy`] into Daytona's `networkBlockAll` and `networkAllowList`.
//!
//! Daytona restricts egress by CIDR block only, and at most ten of them. Anything it cannot carry is refused with
//! [`LeaseError::InvalidNetworkPolicy`] instead of approximated. A host rule is accepted only when it names an IP
//! literal, which becomes a single-address block. A policy that allows nothing blocks all egress.

use std::net::IpAddr;

use trogon_sandbox_contract::{DomainRuleAction, LeaseError, NetworkPolicy, PatternTarget};

use crate::DaytonaNetwork;
use crate::constants::{MAX_ALLOW_ENTRIES, RESTRICTED_EGRESS_UNVERIFIED};

fn invalid(reason: impl Into<String>) -> LeaseError {
    LeaseError::InvalidNetworkPolicy { reason: reason.into() }
}

fn single_address(address: IpAddr) -> String {
    match address {
        IpAddr::V4(_) => format!("{address}/32"),
        IpAddr::V6(_) => format!("{address}/128"),
    }
}

fn blocked() -> DaytonaNetwork {
    DaytonaNetwork {
        block_all: Some(true),
        allow_list: None,
    }
}

/// Refuses a policy that restricts egress, before any sandbox exists. The worker cannot verify that Daytona enforces
/// a restriction, so it will not claim one: only an absent policy or [`NetworkPolicy::AllowAll`] is admitted.
pub fn require_unrestricted_egress(policy: Option<&NetworkPolicy>) -> Result<(), LeaseError> {
    match policy {
        None | Some(NetworkPolicy::AllowAll) => Ok(()),
        Some(NetworkPolicy::DenyAll | NetworkPolicy::UserDefined { .. }) => Err(invalid(RESTRICTED_EGRESS_UNVERIFIED)),
    }
}

/// `None` leaves Daytona's default, which is unrestricted egress, the same as [`NetworkPolicy::AllowAll`].
pub fn translate_network_policy(policy: Option<&NetworkPolicy>) -> Result<DaytonaNetwork, LeaseError> {
    let (rules, allow_ranges, deny_ranges, denied_domains, opaque_tunnels, request_rules) = match policy {
        None | Some(NetworkPolicy::AllowAll) => return Ok(DaytonaNetwork::default()),
        Some(NetworkPolicy::DenyAll) => return Ok(blocked()),
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
        return Err(invalid("Daytona cannot express denied_domains"));
    }
    if !request_rules.is_empty() {
        return Err(invalid("Daytona cannot express request_rules"));
    }
    if !opaque_tunnels.is_allow() {
        return Err(invalid("Daytona cannot refuse opaque tunnels"));
    }
    if !deny_ranges.is_empty() {
        return Err(invalid("Daytona cannot express deny_ranges"));
    }
    let mut entries: Vec<String> = Vec::new();
    for range in allow_ranges {
        if range.port().is_some() {
            return Err(invalid(format!("Daytona cannot restrict by port: {range}")));
        }
        entries.push(range.cidr().as_str().to_owned());
    }
    for rule in rules {
        if !matches!(rule.action, DomainRuleAction::PassThrough) {
            return Err(invalid(format!(
                "Daytona cannot transform or forward traffic for {}",
                rule.domain
            )));
        }
        if rule.domain.port().is_some() {
            return Err(invalid(format!("Daytona cannot restrict by port: {}", rule.domain)));
        }
        let address = match rule.domain.target() {
            PatternTarget::Exact(host) => host.as_ip(),
            PatternTarget::Any | PatternTarget::Subdomains(_) => None,
        };
        let Some(address) = address else {
            return Err(invalid(format!(
                "Daytona restricts egress by IP address, so {} is not expressible",
                rule.domain
            )));
        };
        entries.push(single_address(address));
    }
    entries.sort();
    entries.dedup();
    if entries.is_empty() {
        return Ok(blocked());
    }
    if entries.len() > MAX_ALLOW_ENTRIES {
        return Err(invalid(format!(
            "Daytona accepts at most {MAX_ALLOW_ENTRIES} allowed CIDR blocks, and the policy needs {}",
            entries.len()
        )));
    }
    Ok(DaytonaNetwork {
        block_all: None,
        allow_list: Some(entries.join(",")),
    })
}

#[cfg(test)]
mod tests;

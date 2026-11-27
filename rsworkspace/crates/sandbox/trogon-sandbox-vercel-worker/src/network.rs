//! Translation of the wire [`NetworkPolicy`] into Vercel's sandbox `networkPolicy`.
//!
//! Vercel takes `{"mode": "deny-all"}` or an object with `allow` (domain names) and `subnets.allow` /
//! `subnets.deny` (CIDR blocks). Anything Vercel cannot carry is refused with [`LeaseError::InvalidNetworkPolicy`]
//! instead of approximated. A policy that names only ranges is passed through as written; Vercel then allows those
//! ranges and refuses every hostname.

use serde_json::{Map, Value};
use trogon_sandbox_contract::{DomainRuleAction, IpRange, LeaseError, NetworkPolicy};

use crate::json_object::object;

fn invalid(reason: impl Into<String>) -> LeaseError {
    LeaseError::InvalidNetworkPolicy { reason: reason.into() }
}

fn sorted_ranges(ranges: &[IpRange]) -> Vec<String> {
    let mut texts: Vec<String> = ranges.iter().map(|range| range.as_str().to_owned()).collect();
    texts.sort();
    texts.dedup();
    texts
}

fn deny_all() -> Value {
    object([("mode", Value::from("deny-all"))])
}

/// `None` leaves Vercel's default, which is unrestricted egress, the same as [`NetworkPolicy::AllowAll`].
pub fn translate_network_policy(policy: Option<&NetworkPolicy>) -> Result<Option<Value>, LeaseError> {
    let (rules, allow_ranges, deny_ranges, denied_domains, opaque_tunnels, request_rules) = match policy {
        None | Some(NetworkPolicy::AllowAll) => return Ok(None),
        Some(NetworkPolicy::DenyAll) => return Ok(Some(deny_all())),
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
        return Err(invalid("Vercel cannot express denied_domains"));
    }
    if !request_rules.is_empty() {
        return Err(invalid("Vercel cannot express request_rules"));
    }
    if !opaque_tunnels.is_allow() {
        return Err(invalid("Vercel cannot refuse opaque tunnels"));
    }

    let mut domains: Vec<String> = Vec::new();
    for rule in rules {
        if !matches!(rule.action, DomainRuleAction::PassThrough) {
            return Err(invalid(format!(
                "Vercel cannot transform or forward traffic for {}",
                rule.domain
            )));
        }
        if rule.domain.port().is_some() {
            return Err(invalid(format!("Vercel cannot restrict by port: {}", rule.domain)));
        }
        domains.push(rule.domain.to_string());
    }
    domains.sort();
    domains.dedup();
    let allow = sorted_ranges(allow_ranges);
    let deny = sorted_ranges(deny_ranges);

    if domains.is_empty() && allow.is_empty() {
        return Ok(Some(deny_all()));
    }
    let mut out = Map::new();
    if !domains.is_empty() {
        out.insert("allow".to_owned(), Value::from(domains));
    }
    let mut subnets = Map::new();
    if !allow.is_empty() {
        subnets.insert("allow".to_owned(), Value::from(allow));
    }
    if !deny.is_empty() {
        subnets.insert("deny".to_owned(), Value::from(deny));
    }
    if !subnets.is_empty() {
        out.insert("subnets".to_owned(), Value::Object(subnets));
    }
    Ok(Some(Value::Object(out)))
}

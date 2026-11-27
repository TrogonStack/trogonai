//! Policy decision logic: given a [`NetworkPolicy`], an optional sniffed SNI
//! host name, and a destination address, decide whether to allow or deny a
//! TCP connection attempt.

use crate::host_kind::{HostClassification, HostKind};
use std::net::IpAddr;

use ipnet::IpNet;
use trogon_sandbox_contract::{AllowRange, DomainRule, DomainRuleAction, IpRange, NetworkPolicy};
use trogon_sandbox_contract::{DeniedDomain, DomainPattern, Host, Port};
use trogon_sandbox_protocol::{denied_domain_matches, exact_denied_domain, exact_pattern, pattern_matches};

use crate::resolve::embedded_ipv4;

/// Outcome of evaluating a [`NetworkPolicy`] against a connection attempt.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum Decision {
    Allow,
    Deny,
}

impl Decision {
    pub fn is_allow(self) -> bool {
        matches!(self, Decision::Allow)
    }
}

/// What a policy says about a destination. `Unlisted` means no rule allows
/// it and none denies it: the default deny applies unless a decider rules on
/// it. `Deny` is final, whoever is asking.
#[derive(Debug, Clone, Eq, PartialEq)]
pub enum Ruling {
    Allow(Option<DomainRuleAction>),
    Deny,
    Unlisted,
}

impl Ruling {
    fn default_deny(self) -> (Decision, Option<DomainRuleAction>) {
        match self {
            Ruling::Allow(action) => (Decision::Allow, action),
            Ruling::Deny | Ruling::Unlisted => (Decision::Deny, None),
        }
    }
}

/// Decides whether a TCP connection attempt is allowed under `policy`.
///
/// `sni` is the host name sniffed from a TLS ClientHello, if any (see
/// [`crate::sni::extract_sni`]). `dest_ip`/`dest_port` are the connection's
/// real destination, resolved by the caller before invoking this function.
pub fn decide(policy: &NetworkPolicy, sni: Option<&str>, dest_ip: IpAddr, dest_port: u16) -> Decision {
    decide_with_action(policy, sni, dest_ip, dest_port).0
}

/// Identical to [`decide`], but also returns the matched domain rule's
/// [`DomainRuleAction`] when the decision was reached via an SNI domain-rule
/// match. `None` means either the decision was `Deny`, or `Allow` was reached
/// via an IP-range match or `AllowAll` (no domain rule was involved).
pub fn decide_with_action(
    policy: &NetworkPolicy,
    sni: Option<&str>,
    dest_ip: IpAddr,
    dest_port: u16,
) -> (Decision, Option<DomainRuleAction>) {
    rule(policy, sni, dest_ip, dest_port).default_deny()
}

/// [`decide_with_action`] without the default deny applied, so a caller can
/// tell a destination the policy forbids from one it simply does not list.
pub fn rule(policy: &NetworkPolicy, sni: Option<&str>, dest_ip: IpAddr, dest_port: u16) -> Ruling {
    match policy {
        NetworkPolicy::AllowAll => Ruling::Allow(None),
        NetworkPolicy::DenyAll => Ruling::Deny,
        NetworkPolicy::UserDefined {
            rules,
            allow_ranges,
            deny_ranges,
            denied_domains,
            ..
        } => decide_user_defined(
            rules,
            allow_ranges,
            deny_ranges,
            denied_domains,
            sni,
            dest_ip,
            dest_port,
        ),
    }
}

/// Pre-resolution screen for a destination named by an explicit proxy client,
/// evaluated before any DNS lookup so a denied name never triggers one.
/// `Deny` is final. For a domain, `Allow` means no rule denies the name by
/// itself; the connection is still evaluated again with
/// [`decide_with_action`] once the name has been resolved, which is where
/// deny ranges and allow ranges apply. An IP literal is decided in full here.
pub fn decide_host(policy: &NetworkPolicy, host: &Host, port: u16) -> Decision {
    match rule_host(policy, host, port) {
        Ruling::Allow(_) => Decision::Allow,
        Ruling::Deny => Decision::Deny,
        Ruling::Unlisted => match host.kind() {
            HostKind::Domain(_) if allows_by_address(policy, port) => Decision::Allow,
            _ => Decision::Deny,
        },
    }
}

/// [`decide_host`] without the default deny, and without the allow-range
/// leniency for names: a name no rule lists is `Unlisted`.
pub fn rule_host(policy: &NetworkPolicy, host: &Host, port: u16) -> Ruling {
    match policy {
        NetworkPolicy::AllowAll => Ruling::Allow(None),
        NetworkPolicy::DenyAll => Ruling::Deny,
        NetworkPolicy::UserDefined {
            rules,
            allow_ranges,
            deny_ranges,
            denied_domains,
            ..
        } => match host.kind() {
            HostKind::Ip(ip) => decide_user_defined(rules, allow_ranges, deny_ranges, denied_domains, None, ip, port),
            HostKind::Domain(_) => {
                if denied_domains
                    .iter()
                    .any(|denied| denied_domain_matches(denied, host, port))
                {
                    Ruling::Deny
                } else if let Some(rule) = rules.iter().find(|rule| domain_matches(&rule.domain, host, port)) {
                    Ruling::Allow(Some(rule.action.clone()))
                } else {
                    Ruling::Unlisted
                }
            }
        },
    }
}

/// Whether resolving a name the policy does not list can still end in an
/// allow on `port`, because an allow range open on that port might cover the
/// address it resolves to. When none can, the name is refused unresolved: a
/// DNS query for a name about to be denied would carry the name, and anything
/// a workload encoded in it, to whoever answers for its zone.
pub fn allows_by_address(policy: &NetworkPolicy, port: u16) -> bool {
    matches!(
        policy,
        NetworkPolicy::UserDefined { allow_ranges, .. } if allow_ranges.iter().any(|range| range.allows_port(port))
    )
}

/// Whether the policy forbids dialing `addr` on `port` whatever name led
/// there: a deny range or a denied IP literal covers it.
pub fn refuses_address(policy: &NetworkPolicy, addr: IpAddr, port: u16) -> bool {
    let NetworkPolicy::UserDefined {
        deny_ranges,
        denied_domains,
        ..
    } = policy
    else {
        return matches!(policy, NetworkPolicy::DenyAll);
    };
    let carried = embedded_ipv4(addr).map(IpAddr::V4);
    std::iter::once(addr).chain(carried).any(|candidate| {
        let host = Host::from(candidate);
        any_range_contains(deny_ranges, candidate)
            || denied_domains
                .iter()
                .any(|denied| denied_domain_matches(denied, &host, port))
    })
}

/// The policy with `host:port` added as an exact allow rule (`Allow`) or an
/// exact denied domain (`Deny`), or `None` when the policy cannot or need not
/// change: it is not user-defined, or already carries that exact entry.
pub fn amended(policy: &NetworkPolicy, host: &Host, port: Port, decision: Decision) -> Option<NetworkPolicy> {
    let NetworkPolicy::UserDefined {
        rules,
        allow_ranges,
        deny_ranges,
        denied_domains,
        opaque_tunnels,
        request_rules,
    } = policy
    else {
        return None;
    };
    let mut rules = rules.clone();
    let mut denied_domains = denied_domains.clone();
    match decision {
        Decision::Allow => {
            let domain = exact_pattern(host, port).ok()?;
            if rules.iter().any(|rule| rule.domain == domain) {
                return None;
            }
            rules.push(DomainRule {
                domain,
                action: DomainRuleAction::PassThrough,
            });
        }
        Decision::Deny => {
            let denied = exact_denied_domain(host, port).ok()?;
            if denied_domains.contains(&denied) {
                return None;
            }
            denied_domains.push(denied);
        }
    }
    Some(NetworkPolicy::UserDefined {
        rules,
        allow_ranges: allow_ranges.clone(),
        deny_ranges: deny_ranges.clone(),
        denied_domains,
        opaque_tunnels: *opaque_tunnels,
        request_rules: request_rules.clone(),
    })
}

/// Evaluation order, first hit wins: deny ranges, denied domains, allow
/// domain rules, allow ranges, default deny. A sniffed name that cannot be
/// canonicalised is denied outright rather than falling through to the IP
/// ranges, since the name is attacker-controlled and unvettable.
fn decide_user_defined(
    rules: &[DomainRule],
    allow_ranges: &[AllowRange],
    deny_ranges: &[IpRange],
    denied_domains: &[DeniedDomain],
    sni: Option<&str>,
    dest_ip: IpAddr,
    dest_port: u16,
) -> Ruling {
    if any_range_contains(deny_ranges, dest_ip) {
        return Ruling::Deny;
    }

    let dest_host = Host::from(dest_ip);
    let sni_host = match sni.map(Host::new) {
        Some(Ok(host)) => Some(host),
        Some(Err(_)) => return Ruling::Deny,
        None => None,
    };
    let candidates = || sni_host.iter().chain(std::iter::once(&dest_host));

    if denied_domains
        .iter()
        .any(|denied| candidates().any(|host| denied_domain_matches(denied, host, dest_port)))
    {
        return Ruling::Deny;
    }

    if let Some(rule) = first_matching_rule(rules, sni_host.as_ref(), &dest_host, dest_port) {
        return Ruling::Allow(Some(rule.action.clone()));
    }

    if allow_ranges
        .iter()
        .any(|range| range.allows_port(dest_port) && ip_range_contains(range.cidr(), dest_ip))
    {
        return Ruling::Allow(None);
    }

    Ruling::Unlisted
}

fn first_matching_rule<'a>(
    rules: &'a [DomainRule],
    sni_host: Option<&Host>,
    dest_host: &Host,
    dest_port: u16,
) -> Option<&'a DomainRule> {
    rules.iter().find(|rule| {
        sni_host
            .into_iter()
            .chain(std::iter::once(dest_host))
            .any(|host| domain_matches(&rule.domain, host, dest_port))
    })
}

/// Whether an allow reached through `sni` rests on the name the client
/// presented, as opposed to the destination address or `AllowAll`. Only then
/// can the name be used to reach a destination the policy never approved.
/// Call it for connections [`decide_with_action`] allowed.
pub fn allow_rests_on_sni(policy: &NetworkPolicy, sni: Option<&str>, dest_ip: IpAddr, dest_port: u16) -> bool {
    let (NetworkPolicy::UserDefined { rules, .. }, Some(Ok(sni_host))) = (policy, sni.map(Host::new)) else {
        return false;
    };
    first_matching_rule(rules, Some(&sni_host), &Host::from(dest_ip), dest_port)
        .is_some_and(|rule| domain_matches(&rule.domain, &sni_host, dest_port))
}

/// Whether any of `ranges` covers `ip`, read as the IPv4 address it spells
/// when it is an IPv4-mapped IPv6 address.
pub fn addresses_contain(ranges: &[IpRange], ip: IpAddr) -> bool {
    let folded = crate::fold_mapped::fold_mapped(ip);
    any_range_contains(ranges, ip) || any_range_contains(ranges, folded)
}

/// Whether the policy names `ip` itself as allowed on `port`: an allow range of
/// exactly that one address, or an exact rule for the address literal. An allow like
/// that does not depend on which name the client presented.
pub fn address_allowed_exactly(policy: &NetworkPolicy, ip: IpAddr, port: u16) -> bool {
    let NetworkPolicy::UserDefined {
        rules, allow_ranges, ..
    } = policy
    else {
        return matches!(policy, NetworkPolicy::AllowAll);
    };
    let ip = crate::fold_mapped::fold_mapped(ip);
    let single = allow_ranges.iter().any(|range| {
        let cidr = range.cidr();
        range.allows_port(port)
            && IpNet::new(cidr.address(), cidr.prefix_len())
                .is_ok_and(|net| net.prefix_len() == net.max_prefix_len() && net.contains(&ip))
    });
    single
        || rules
            .iter()
            .any(|rule| domain_matches(&rule.domain, &Host::from(ip), port))
}

fn any_range_contains(ranges: &[IpRange], ip: IpAddr) -> bool {
    ranges.iter().any(|range| ip_range_contains(range, ip))
}

fn ip_range_contains(range: &IpRange, ip: IpAddr) -> bool {
    IpNet::new(range.address(), range.prefix_len()).is_ok_and(|net| net.contains(&ip))
}

/// Matches a pattern against a canonical destination host and port. A pattern
/// with a single leading `*.` matches any strict subdomain (`*.example.com`
/// matches `api.example.com` but not `example.com` itself and not
/// `evil-example.com`) and never an IP literal. A pattern with a `:port`
/// suffix matches only that port.
pub fn domain_matches(pattern: &DomainPattern, host: &Host, port: u16) -> bool {
    pattern_matches(pattern, host, port)
}

#[cfg(test)]
mod tests;

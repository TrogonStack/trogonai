use std::net::IpAddr;

use trogon_sandbox_contract::{DeniedDomain, DomainPattern, DomainPatternError, Host, PatternTarget, Port};

/// The textual form of a pattern's target: `*`, a host, or `*.name`.
fn target_text(target: &PatternTarget) -> String {
    match target {
        PatternTarget::Any => "*".to_string(),
        PatternTarget::Exact(host) => host.to_string(),
        PatternTarget::Subdomains(suffix) => format!("*.{suffix}"),
    }
}

/// The same pattern restricted to `port`.
pub fn pattern_with_port(pattern: &DomainPattern, port: Port) -> Result<DomainPattern, DomainPatternError> {
    DomainPattern::new(&format!("{}:{port}", target_text(pattern.target())))
}

/// The same pattern spanning every port.
pub fn pattern_without_port(pattern: &DomainPattern) -> Result<DomainPattern, DomainPatternError> {
    DomainPattern::new(&target_text(pattern.target()))
}

/// The pattern matching exactly `host` on exactly `port`.
pub fn exact_pattern(host: &Host, port: Port) -> Result<DomainPattern, DomainPatternError> {
    DomainPattern::new(&format!("{host}:{port}"))
}

/// The entry denying exactly `host` on exactly `port`.
pub fn exact_denied_domain(host: &Host, port: Port) -> Result<DeniedDomain, DomainPatternError> {
    DeniedDomain::new(&format!("{host}:{port}"))
}

/// The address of an IP literal target, or `None` for names and the match-everything target.
pub fn target_ip_literal(target: &PatternTarget) -> Option<IpAddr> {
    match target {
        PatternTarget::Exact(host) => host.as_ip(),
        PatternTarget::Any | PatternTarget::Subdomains(_) => None,
    }
}

/// The host-name part of a target as `example.com` or `*.example.com`.
///
/// IP literals and the match-everything target have no DNS name, so they give `None`.
pub fn target_dns_name(target: &PatternTarget) -> Option<String> {
    match target {
        PatternTarget::Exact(host) => host.as_domain().map(ToString::to_string),
        PatternTarget::Subdomains(suffix) => Some(format!("*.{suffix}")),
        PatternTarget::Any => None,
    }
}

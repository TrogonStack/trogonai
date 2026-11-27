use trogon_sandbox_contract::{DeniedDomain, DomainName, DomainPattern, Host, PatternTarget, Port};

/// Reports whether `host` is a label sequence below `suffix`, never `suffix` itself.
fn is_strict_subdomain_of(host: &DomainName, suffix: &DomainName) -> bool {
    host.as_str()
        .strip_suffix(suffix.as_str())
        .is_some_and(|rest| rest.len() > 1 && rest.ends_with('.'))
}

/// Reports whether a pattern with `target` and `port` names the destination `host:actual_port`.
///
/// A pattern without a port matches every port. `*.example.com` matches strict subdomains only and never an IP
/// literal.
pub fn destination_matches(target: &PatternTarget, port: Option<Port>, host: &Host, actual_port: u16) -> bool {
    if port.is_some_and(|expected| expected.get() != actual_port) {
        return false;
    }
    match target {
        PatternTarget::Any => true,
        PatternTarget::Exact(expected) => expected == host,
        PatternTarget::Subdomains(suffix) => host
            .as_domain()
            .is_some_and(|name| is_strict_subdomain_of(name, suffix)),
    }
}

/// Reports whether `pattern` names the destination `host:port`.
pub fn pattern_matches(pattern: &DomainPattern, host: &Host, port: u16) -> bool {
    destination_matches(pattern.target(), pattern.port(), host, port)
}

/// Reports whether the deny entry `denied` names the destination `host:port`.
pub fn denied_domain_matches(denied: &DeniedDomain, host: &Host, port: u16) -> bool {
    destination_matches(denied.target(), denied.port(), host, port)
}

/// Reports whether every destination named by `target:port` is also named by `other_target:other_port`.
///
/// `api.example.com` is covered by `*.example.com`, and so is `*.api.example.com`, but `*.example.com` is not covered
/// by an exact host. A pattern without a port spans every port, so only a covering pattern without a port covers it.
pub fn destination_is_covered_by(
    target: &PatternTarget,
    port: Option<Port>,
    other_target: &PatternTarget,
    other_port: Option<Port>,
) -> bool {
    if let Some(expected) = other_port
        && port != Some(expected)
    {
        return false;
    }
    match (target, other_target) {
        (_, PatternTarget::Any) => true,
        (PatternTarget::Exact(host), PatternTarget::Exact(expected)) => host == expected,
        (PatternTarget::Exact(host), PatternTarget::Subdomains(suffix)) => host
            .as_domain()
            .is_some_and(|name| is_strict_subdomain_of(name, suffix)),
        (PatternTarget::Any, _) | (PatternTarget::Subdomains(_), PatternTarget::Exact(_)) => false,
        (PatternTarget::Subdomains(base), PatternTarget::Subdomains(suffix)) => {
            base == suffix || is_strict_subdomain_of(base, suffix)
        }
    }
}

/// Reports whether every destination `pattern` names is also named by `other`.
pub fn pattern_is_covered_by(pattern: &DomainPattern, other: &DomainPattern) -> bool {
    destination_is_covered_by(pattern.target(), pattern.port(), other.target(), other.port())
}

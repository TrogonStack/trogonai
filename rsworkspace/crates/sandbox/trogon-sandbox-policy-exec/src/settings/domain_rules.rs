//! The string checks a domain list entry goes through, so an entry is accepted
//! or refused by its spelling alone. An entry that passes them but names no
//! host this runtime can match stands for nothing and is dropped.

use std::net::{Ipv4Addr, Ipv6Addr};

use trogon_sandbox_contract::{DeniedDomain, DomainPattern};

pub(super) fn is_ip(text: &str) -> bool {
    text.parse::<Ipv4Addr>().is_ok() || is_ipv6(text)
}

fn is_ipv6(text: &str) -> bool {
    let (address, zone) = match text.split_once('%') {
        Some((address, zone)) => (address, Some(zone)),
        None => (text, None),
    };
    if let Some(zone) = zone
        && (zone.is_empty()
            || !zone
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '.' | ':')))
    {
        return false;
    }
    address.parse::<Ipv6Addr>().is_ok()
}

fn strip_brackets(host: &str) -> &str {
    if host.starts_with('[') && host.ends_with(']') && host.len() >= 2 {
        &host[1..host.len() - 1]
    } else {
        host
    }
}

fn drop_zone(host: &str) -> &str {
    host.find('%').map_or(host, |at| &host[..at])
}

/// The host in its canonical spelling: lowercase, an IPv6 address bracketed.
fn canonicalize_host(host: &str) -> Option<String> {
    let bare = strip_brackets(host);
    let bracketed = if is_ipv6(bare) {
        format!("[{bare}]")
    } else {
        bare.to_string()
    };
    let url = url::Url::parse(&format!("http://{bracketed}/")).ok()?;
    match url.host()? {
        url::Host::Ipv6(address) => Some(match address.to_ipv4_mapped() {
            Some(mapped) => mapped.to_string(),
            None => address.to_string(),
        }),
        url::Host::Ipv4(address) => Some(address.to_string()),
        url::Host::Domain(name) => Some(name.strip_suffix('.').unwrap_or(name).to_string()),
    }
}

fn parse_port_suffix(suffix: &str) -> Option<u16> {
    let bytes = suffix.as_bytes();
    let valid = (1..=5).contains(&bytes.len()) && bytes[0] != b'0' && bytes.iter().all(u8::is_ascii_digit);
    if !valid {
        return None;
    }
    suffix.parse::<u16>().ok()
}

pub(super) struct Split {
    pub(super) host: String,
    pub(super) port: Option<u16>,
}

/// Splits a trailing `:port` off a pattern when the rest is a host.
pub(super) fn split_port(pattern: &str) -> Split {
    let whole = |host: &str| Split {
        host: host.to_string(),
        port: None,
    };
    if pattern.starts_with('[') {
        let Some(close) = pattern.find(']') else {
            return whole(pattern);
        };
        let inner = drop_zone(&pattern[1..close]);
        let host = canonicalize_host(inner).unwrap_or_else(|| inner.to_string());
        let rest = &pattern[close + 1..];
        if rest.is_empty() {
            return Split { host, port: None };
        }
        return match parse_port_suffix(rest.strip_prefix(':').unwrap_or("")) {
            Some(port) => Split { host, port: Some(port) },
            None => whole(pattern),
        };
    }
    let Some(last) = pattern.rfind(':') else {
        return whole(pattern);
    };
    if pattern.find(':') != Some(last) {
        let host = drop_zone(pattern);
        return whole(&canonicalize_host(host).unwrap_or_else(|| host.to_string()));
    }
    match parse_port_suffix(&pattern[last + 1..]) {
        Some(port) => Split {
            host: pattern[..last].to_string(),
            port: Some(port),
        },
        None => whole(pattern),
    }
}

/// Whether a host part is an IPv6 address or a dotted name with no empty edge label.
pub(super) fn is_valid_domain_pattern(val: &str) -> bool {
    if is_ipv6(val) {
        return true;
    }
    if val.contains("://") || val.contains('/') || val.contains(':') {
        return false;
    }
    if val == "localhost" {
        return true;
    }
    if let Some(domain) = val.strip_prefix("*.") {
        if !domain.contains('.') || domain.starts_with('.') || domain.ends_with('.') {
            return false;
        }
        return domain.split('.').all(|part| !part.is_empty());
    }
    if val.contains('*') {
        return false;
    }
    val.contains('.') && !val.starts_with('.') && !val.ends_with('.')
}

/// Whether an entry with more than one colon is bracketed, as an IPv6 address must be.
fn has_valid_ipv6_bracketing(val: &str) -> bool {
    let Some(first) = val.find(':') else {
        return true;
    };
    let multi_colon = val[first + 1..].contains(':');
    !multi_colon || val.starts_with('[')
}

/// Whether an allow-list entry is well bracketed and has a valid host part.
pub(super) fn is_valid_allowed_entry(val: &str) -> bool {
    has_valid_ipv6_bracketing(val) && is_valid_domain_pattern(&split_port(val).host)
}

/// Whether a deny-list entry is well bracketed and is `*` or has a valid host part.
pub(super) fn is_valid_denied_entry(val: &str) -> bool {
    if !has_valid_ipv6_bracketing(val) {
        return false;
    }
    let host = split_port(val).host;
    host == "*" || is_valid_domain_pattern(&host)
}

fn spelled(split: &Split) -> String {
    let host = if is_ipv6(&split.host) && !split.host.starts_with('[') {
        format!("[{}]", split.host)
    } else {
        split.host.clone()
    };
    match split.port {
        Some(port) => format!("{host}:{port}"),
        None => host,
    }
}

/// The allow-list pattern an accepted entry stands for, if any host can match it.
pub(super) fn allowed_pattern(val: &str) -> Option<DomainPattern> {
    DomainPattern::new(&spelled(&split_port(val))).ok()
}

pub(super) fn denied_pattern(val: &str) -> Option<DeniedDomain> {
    DeniedDomain::new(&spelled(&split_port(val))).ok()
}

/// The pattern an accepted entry of a list without ports stands for.
pub(super) fn plain_pattern(val: &str) -> Option<DomainPattern> {
    let host = if is_ipv6(val) {
        format!("[{}]", drop_zone(val))
    } else {
        val.to_string()
    };
    DomainPattern::new(&host).ok()
}

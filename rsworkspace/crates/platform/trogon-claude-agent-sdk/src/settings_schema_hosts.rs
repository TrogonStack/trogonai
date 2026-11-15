//! Port of the URL host checks the settings schema applies (upstream `g6`, `HJe`, `$6`, `R3e` and
//! the IPv6 helpers behind them).

use std::sync::LazyLock;

use regex::Regex;
use url::Url;

use crate::constants::{
    SETTINGS_METADATA_ADDRESSES, SETTINGS_METADATA_HOSTS, SETTINGS_PATTERN_IPV4_SEGMENT, SETTINGS_PATTERN_IPV6_SEGMENT,
    SETTINGS_PATTERN_IPV6_ZONE,
};

fn ipv4_pattern() -> String {
    let segment = SETTINGS_PATTERN_IPV4_SEGMENT;
    format!("^(?:{segment}\\.){{3}}{segment}$")
}

fn ipv6_pattern() -> String {
    let v4_segment = SETTINGS_PATTERN_IPV4_SEGMENT;
    let v4 = format!("(?:{v4_segment}\\.){{3}}{v4_segment}");
    let seg = SETTINGS_PATTERN_IPV6_SEGMENT;
    let zone = SETTINGS_PATTERN_IPV6_ZONE;
    format!(
        "^(?:(?:{seg}:){{7}}(?:{seg}|:)|\
         (?:{seg}:){{6}}(?:{v4}|:{seg}|:)|\
         (?:{seg}:){{5}}(?::{v4}|(?::{seg}){{1,2}}|:)|\
         (?:{seg}:){{4}}(?:(?::{seg}){{0,1}}:{v4}|(?::{seg}){{1,3}}|:)|\
         (?:{seg}:){{3}}(?:(?::{seg}){{0,2}}:{v4}|(?::{seg}){{1,4}}|:)|\
         (?:{seg}:){{2}}(?:(?::{seg}){{0,3}}:{v4}|(?::{seg}){{1,5}}|:)|\
         (?:{seg}:){{1}}(?:(?::{seg}){{0,4}}:{v4}|(?::{seg}){{1,6}}|:)|\
         (?::(?:(?::{seg}){{0,5}}:{v4}|(?::{seg}){{1,7}}|:)))\
         {zone}"
    )
}

static IPV4: LazyLock<Option<Regex>> = LazyLock::new(|| Regex::new(&ipv4_pattern()).ok());
static IPV6: LazyLock<Option<Regex>> = LazyLock::new(|| Regex::new(&ipv6_pattern()).ok());

fn is_ipv4(host: &str) -> bool {
    IPV4.as_ref().is_some_and(|regex| regex.is_match(host))
}

fn is_ipv6(host: &str) -> bool {
    IPV6.as_ref().is_some_and(|regex| regex.is_match(host))
}

/// Upstream `f6`: loopback, link-local and unspecified IPv4 ranges.
fn is_private_v4(first: u32, second: u32) -> bool {
    first == 127 || (first == 169 && second == 254) || first == 0
}

/// Upstream `z3e`: the sixteen bytes of an IPv6 literal, or `None` when it does not parse.
fn parse_ipv6(text: &str) -> Option<Vec<u32>> {
    let stripped = text.split('%').next().unwrap_or(text).to_lowercase();
    let halves: Vec<&str> = stripped.split("::").collect();
    if halves.len() > 2 {
        return None;
    }
    let split_groups = |half: &str| -> Vec<String> {
        if half.is_empty() {
            Vec::new()
        } else {
            half.split(':').map(str::to_owned).collect()
        }
    };
    let head = split_groups(halves[0]);
    let tail_half = if halves.len() == 2 && !halves[1].is_empty() {
        split_groups(halves[1])
    } else {
        Vec::new()
    };
    let mut groups = if halves.len() == 2 { tail_half } else { head.clone() };
    let mut trailing: Vec<u32> = Vec::new();
    if let Some(last) = groups.last().filter(|last| last.contains('.')).cloned() {
        let parts: Vec<u32> = last
            .split('.')
            .map(|part| part.parse::<u32>().ok().filter(|_| !part.is_empty()))
            .collect::<Option<Vec<u32>>>()?;
        if parts.len() != 4 || parts.iter().any(|part| *part > 255) {
            return None;
        }
        trailing = parts;
        groups.pop();
    }
    let to_bytes = |groups: &[String]| -> Option<Vec<u32>> {
        let mut bytes = Vec::new();
        for group in groups {
            if group.is_empty() || group.len() > 4 || !group.chars().all(|c| matches!(c, '0'..='9' | 'a'..='f')) {
                return None;
            }
            let value = u32::from_str_radix(group, 16).ok()?;
            bytes.push(value >> 8);
            bytes.push(value & 255);
        }
        Some(bytes)
    };
    let leading = to_bytes(if halves.len() == 2 { &head } else { &[] })?;
    let middle = to_bytes(&groups)?;
    let filled = leading.len() + middle.len() + trailing.len();
    if filled > 16 || (halves.len() == 1 && filled != 16) {
        return None;
    }
    let mut bytes = leading;
    bytes.extend(std::iter::repeat_n(0, 16 - filled));
    bytes.extend(middle);
    bytes.extend(trailing);
    Some(bytes)
}

/// Upstream `m6`: the `64:ff9b::/96` NAT64 prefix.
fn has_nat64_prefix(bytes: &[u32]) -> bool {
    bytes[..4] == [0, 100, 255, 155]
}

/// Upstream `j3e`: IPv4 addresses an IPv6 address embeds.
fn embedded_v4(bytes: &[u32]) -> Vec<[u32; 4]> {
    let mut found = Vec::new();
    let tail = |bytes: &[u32]| [bytes[12], bytes[13], bytes[14], bytes[15]];
    if bytes[0] == 32 && bytes[1] == 2 {
        found.push([bytes[2], bytes[3], bytes[4], bytes[5]]);
    }
    let zero_prefix = bytes[..10].iter().all(|byte| *byte == 0);
    let mapped = zero_prefix && bytes[10] == 255 && bytes[11] == 255;
    let compatible = zero_prefix && bytes[10] == 0 && bytes[11] == 0;
    let nat64 = has_nat64_prefix(bytes) && bytes[4..12].iter().all(|byte| *byte == 0);
    let isatap = (bytes[8] == 0 || bytes[8] == 2) && bytes[9] == 0 && bytes[10] == 94 && bytes[11] == 254;
    if mapped || compatible || nat64 || isatap {
        found.push(tail(bytes));
    }
    found
}

/// Upstream `g6`: whether a host names a loopback, link-local, metadata or private destination.
pub(crate) fn is_blocked_host(hostname: &str) -> bool {
    let lowered = hostname.to_lowercase();
    let without_brackets = lowered.strip_prefix('[').unwrap_or(&lowered);
    let without_brackets = without_brackets.strip_suffix(']').unwrap_or(without_brackets);
    let host = without_brackets.strip_suffix('.').unwrap_or(without_brackets);
    if host.is_empty() || host == "localhost" || host.ends_with(".localhost") {
        return true;
    }
    if SETTINGS_METADATA_HOSTS.contains(&host) {
        return true;
    }
    if host.starts_with("instance-data.") && host.ends_with(".compute.internal") {
        return true;
    }
    if is_ipv4(host) {
        if SETTINGS_METADATA_ADDRESSES.contains(&host) {
            return true;
        }
        let parts: Vec<u32> = host.split('.').map(|part| part.parse().unwrap_or(0)).collect();
        return is_private_v4(parts[0], parts[1]);
    }
    if !is_ipv6(host) {
        return false;
    }
    let Some(bytes) = parse_ipv6(host) else {
        return true;
    };
    if has_nat64_prefix(&bytes) && bytes[4] == 0 && bytes[5] == 1 {
        return true;
    }
    if bytes.iter().all(|byte| *byte == 0) {
        return true;
    }
    if bytes[..15].iter().all(|byte| *byte == 0) && bytes[15] == 1 {
        return true;
    }
    if host == "fd00:ec2::254" {
        return true;
    }
    if bytes[0] == 254 && (128..=191).contains(&bytes[1]) {
        return true;
    }
    embedded_v4(&bytes).iter().any(|address| {
        is_private_v4(address[0], address[1])
            || SETTINGS_METADATA_ADDRESSES
                .contains(&format!("{}.{}.{}.{}", address[0], address[1], address[2], address[3]).as_str())
    })
}

fn hostname_of(url: &Url) -> &str {
    url.host_str().unwrap_or("")
}

/// Upstream `HJe`: an https URL whose host is not a blocked destination.
pub(crate) fn is_public_https_url(text: &str) -> bool {
    Url::parse(text).is_ok_and(|url| url.scheme() == "https" && !is_blocked_host(hostname_of(&url)))
}

/// Upstream `R3e`: an https URL with a host.
pub(crate) fn is_https_with_host(text: &str) -> bool {
    Url::parse(text).is_ok_and(|url| url.scheme() == "https" && !hostname_of(&url).is_empty())
}

/// Upstream `$6`: an http or https URL with an authority.
pub(crate) fn is_http_url(text: &str) -> bool {
    let lowered = text.to_ascii_lowercase();
    let after = lowered
        .strip_prefix("https://")
        .or_else(|| lowered.strip_prefix("http://"));
    let Some(after) = after else {
        return false;
    };
    if after.chars().next().is_none_or(|first| first == '/' || first == '\\') {
        return false;
    }
    Url::parse(text).is_ok_and(|url| url.scheme() == "https" || url.scheme() == "http")
}

#[cfg(test)]
#[path = "settings_schema_hosts_tests.rs"]
mod tests;

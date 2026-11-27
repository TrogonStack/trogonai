use std::net::IpAddr;

use crate::{NoProxyCidr, NoProxyEntry};

/// The `NO_PROXY` list: the hosts that skip the parent proxy.
///
/// It is read from a comma separated list. An entry that is `*` is a wildcard, one with a `/` is an address range
/// and is ignored when malformed, and the rest are lowercase host names or addresses. A zone id, the brackets around
/// an IPv6 address, and a trailing `:port` are removed, so a `host:port` entry names the host on every port.
#[derive(Debug, Clone, Default, Eq, PartialEq)]
pub struct NoProxy(Vec<NoProxyEntry>);

impl NoProxy {
    pub fn new(raw: &str) -> Self {
        let mut entries = Vec::new();
        for entry in raw.split(',') {
            let entry = without_zone(entry.trim());
            if entry.is_empty() {
                continue;
            }
            if entry == "*" {
                entries.push(NoProxyEntry::Wildcard);
                continue;
            }
            if entry.contains('/') {
                entries.extend(NoProxyCidr::new(&entry).ok().map(NoProxyEntry::Cidr));
                continue;
            }
            let mut value = entry.to_ascii_lowercase();
            if let Some(inner) = bracketed_host(&value) {
                value = inner;
            }
            if let Some(rest) = value.strip_prefix("*.") {
                value = format!(".{rest}");
            }
            match value.parse::<IpAddr>() {
                Ok(address) => entries.push(NoProxyEntry::Cidr(NoProxyCidr::from(address))),
                Err(_) => {
                    if let Some((head, port)) = value.rsplit_once(':')
                        && !port.is_empty()
                        && port.bytes().all(|byte| byte.is_ascii_digit())
                    {
                        value = head.to_string();
                    }
                    entries.push(NoProxyEntry::Suffix(value));
                }
            }
        }
        Self(entries)
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn entries(&self) -> &[NoProxyEntry] {
        &self.0
    }
}

fn without_zone(entry: &str) -> String {
    match entry.split_once('%') {
        Some((head, tail)) if head.contains(':') => {
            let rest = tail.find('/').map_or("", |at| &tail[at..]);
            format!("{head}{rest}")
        }
        _ => entry.to_string(),
    }
}

fn bracketed_host(value: &str) -> Option<String> {
    let rest = value.strip_prefix('[')?;
    let (inner, tail) = rest.split_once(']')?;
    let tail_ok = tail.is_empty()
        || tail
            .strip_prefix(':')
            .is_some_and(|port| !port.is_empty() && port.bytes().all(|byte| byte.is_ascii_digit()));
    tail_ok.then(|| inner.to_string())
}

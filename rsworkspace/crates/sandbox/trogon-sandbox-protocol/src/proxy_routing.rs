use std::net::IpAddr;

use trogon_sandbox_contract::{
    Host, NoProxy, NoProxyCidr, NoProxyEntry, ParentProxy, ParentProxyUrl, ParentProxyUrlError, ProxyScheme,
};

use crate::{canonical_ip, is_loopback_host};

/// Reports whether `raw` is an absolute URL.
pub fn is_parent_proxy_url(raw: &str) -> bool {
    url::Url::parse(raw).is_ok()
}

/// Reports whether the `NO_PROXY` list sends `host` around the parent proxy.
pub fn no_proxy_bypasses(no_proxy: &NoProxy, host: &Host) -> bool {
    let name = host.to_string();
    no_proxy.entries().iter().any(|entry| match entry {
        NoProxyEntry::Wildcard => true,
        NoProxyEntry::Cidr(cidr) => host.as_ip().is_some_and(|ip| cidr_contains(cidr, ip)),
        NoProxyEntry::Suffix(suffix) => suffix_covers(suffix, &name),
    })
}

fn suffix_covers(suffix: &str, name: &str) -> bool {
    match suffix.strip_prefix('.') {
        Some(bare) => name == bare || name.ends_with(suffix),
        None => name == suffix || name.ends_with(&format!(".{suffix}")),
    }
}

fn cidr_contains(cidr: &NoProxyCidr, ip: IpAddr) -> bool {
    match (cidr.network(), canonical_ip(ip)) {
        (IpAddr::V4(network), IpAddr::V4(ip)) => {
            prefix_matches(u32::from(network).into(), u32::from(ip).into(), cidr.prefix(), 32)
        }
        (IpAddr::V6(network), IpAddr::V6(ip)) => {
            prefix_matches(u128::from(network), u128::from(ip), cidr.prefix(), 128)
        }
        _ => false,
    }
}

fn prefix_matches(network: u128, ip: u128, prefix: u8, width: u32) -> bool {
    let prefix = u32::from(prefix);
    if prefix == 0 {
        return true;
    }
    let shift = width.saturating_sub(prefix);
    (network >> shift) == (ip >> shift)
}

/// Reports whether `host` skips the parent proxy. Loopback never goes through one, whatever `NO_PROXY` says.
pub fn parent_proxy_bypasses(proxy: &ParentProxy, host: &Host) -> bool {
    is_loopback_host(host) || no_proxy_bypasses(proxy.no_proxy(), host)
}

/// The proxy for a destination reached over `scheme`: for TLS the https one falling back to the http one, for plain
/// HTTP only the http one.
pub fn select_parent_proxy(proxy: &ParentProxy, scheme: ProxyScheme) -> Option<&ParentProxyUrl> {
    match scheme {
        ProxyScheme::Https => proxy.https().or(proxy.http()),
        ProxyScheme::Http => proxy.http(),
    }
}

/// The proxy a destination is reached through, or `None` when it skips the parent proxy.
pub fn parent_proxy_route_for<'a>(
    proxy: &'a ParentProxy,
    host: &Host,
    scheme: ProxyScheme,
) -> Option<&'a ParentProxyUrl> {
    if parent_proxy_bypasses(proxy, host) {
        None
    } else {
        select_parent_proxy(proxy, scheme)
    }
}

/// Resolves explicit settings first and the environment second (`HTTP_PROXY`, `HTTPS_PROXY`, `NO_PROXY`, either
/// case).
///
/// `https` falls back to `http`. A set URL that does not parse is an error, never ignored, because ignoring it would
/// send traffic directly that the operator meant to route through the proxy. With nothing set there is no parent
/// proxy.
pub fn resolve_parent_proxy(
    http: Option<&str>,
    https: Option<&str>,
    no_proxy: Option<&str>,
    env: impl Fn(&str) -> Option<String>,
) -> Result<Option<ParentProxy>, ParentProxyUrlError> {
    let lookup = |upper: &str, lower: &str| env(upper).or_else(|| env(lower));
    let http = http.map(str::to_string).or_else(|| lookup("HTTP_PROXY", "http_proxy"));
    let https = https
        .map(str::to_string)
        .or_else(|| lookup("HTTPS_PROXY", "https_proxy"))
        .or_else(|| http.clone());
    let no_proxy = no_proxy
        .map(str::to_string)
        .or_else(|| lookup("NO_PROXY", "no_proxy"))
        .unwrap_or_default();
    let parse = |raw: Option<String>| {
        raw.filter(|raw| !raw.is_empty())
            .map(|raw| ParentProxyUrl::new(&raw))
            .transpose()
    };
    Ok(ParentProxy::new(parse(http)?, parse(https)?, NoProxy::new(&no_proxy)).ok())
}

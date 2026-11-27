use std::net::IpAddr;

use trogon_sandbox_contract::Host;

/// The address with an IPv4-mapped IPv6 address read as the IPv4 address it wraps.
pub fn canonical_ip(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V6(v6) => v6.to_ipv4_mapped().map_or(ip, IpAddr::V4),
        v4 => v4,
    }
}

/// Reports whether `host` can only be the local machine.
pub fn is_loopback_host(host: &Host) -> bool {
    match host.as_ip() {
        Some(ip) => canonical_ip(ip).is_loopback(),
        None => host.as_domain().is_some_and(|name| is_loopback_name(name.as_str())),
    }
}

/// Reports whether `name` is reserved for the loopback interface: `localhost` and everything under `.localhost`.
pub fn is_loopback_name(name: &str) -> bool {
    let name = name.strip_suffix('.').unwrap_or(name);
    name.eq_ignore_ascii_case("localhost")
        || name
            .len()
            .checked_sub(".localhost".len())
            .and_then(|at| name.get(at..))
            .is_some_and(|tail| tail.eq_ignore_ascii_case(".localhost"))
}

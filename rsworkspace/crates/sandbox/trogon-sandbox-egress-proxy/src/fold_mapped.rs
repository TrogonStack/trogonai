use std::net::IpAddr;

/// The IPv4-mapped form is the IPv4 address it spells.
pub(crate) fn fold_mapped(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V6(v6) => v6.to_ipv4_mapped().map_or(ip, IpAddr::V4),
        IpAddr::V4(_) => ip,
    }
}

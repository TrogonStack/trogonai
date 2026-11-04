//! Classification of resolved addresses (ADR#0085 Decision 7).
//!
//! A permitted *name* may only be dialed at a public address. Every other
//! class is platform-internal or unroutable, and reaching one requires a
//! permit that names the exact address literal. The module is pure so the
//! ranges are unit-tested without a socket.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AddressClass {
    Public,
    /// `127.0.0.0/8`, `::1`.
    Loopback,
    /// `169.254.0.0/16` (includes the cloud metadata address), `fe80::/10`.
    LinkLocal,
    /// RFC 1918 (`10/8`, `172.16/12`, `192.168/16`) and the deprecated IPv6 site-local `fec0::/10`.
    Private,
    /// RFC 6598 shared address space, `100.64.0.0/10`.
    Cgnat,
    /// `fc00::/7`.
    UniqueLocal,
    /// `0.0.0.0/8`, `::`.
    Unspecified,
    Multicast,
    /// `240.0.0.0/4`, which includes the limited broadcast address.
    Reserved,
}

impl AddressClass {
    pub fn is_public(self) -> bool {
        self == Self::Public
    }
}

/// Classifies an address. An IPv6 address that embeds an IPv4 address
/// (IPv4-mapped, the deprecated IPv4-compatible form, NAT64 `64:ff9b::/96`,
/// 6to4 `2002::/16`) takes the class of the IPv4 address it carries, so
/// wrapping a loopback address in IPv6 does not launder it. Teredo is not
/// decoded: it is not a route the proxy host has.
pub fn classify(address: IpAddr) -> AddressClass {
    match address {
        IpAddr::V4(v4) => classify_v4(v4),
        IpAddr::V6(v6) => classify_v6(v6),
    }
}

fn classify_v4(address: Ipv4Addr) -> AddressClass {
    let [a, b, _, _] = address.octets();
    match (a, b) {
        (0, _) => AddressClass::Unspecified,
        (127, _) => AddressClass::Loopback,
        (169, 254) => AddressClass::LinkLocal,
        (10, _) | (192, 168) => AddressClass::Private,
        (172, 16..=31) => AddressClass::Private,
        (100, 64..=127) => AddressClass::Cgnat,
        (224..=239, _) => AddressClass::Multicast,
        (240..=255, _) => AddressClass::Reserved,
        _ => AddressClass::Public,
    }
}

fn classify_v6(address: Ipv6Addr) -> AddressClass {
    if address.is_unspecified() {
        return AddressClass::Unspecified;
    }
    if address.is_loopback() {
        return AddressClass::Loopback;
    }
    let s = address.segments();
    if let Some(v4) = address.to_ipv4_mapped() {
        return classify_v4(v4);
    }
    // IPv4-compatible `::a.b.c.d` and NAT64 `64:ff9b::a.b.c.d`.
    let embedded_in_low_32 = |segments: [u16; 2]| {
        let [hi, lo] = segments;
        let [a, b] = hi.to_be_bytes();
        let [c, d] = lo.to_be_bytes();
        Ipv4Addr::new(a, b, c, d)
    };
    if s[..6].iter().all(|segment| *segment == 0)
        || (s[0] == 0x0064 && s[1] == 0xff9b && s[2..6].iter().all(|x| *x == 0))
    {
        return classify_v4(embedded_in_low_32([s[6], s[7]]));
    }
    // 6to4 carries the IPv4 address in bits 16..48.
    if s[0] == 0x2002 {
        let embedded = classify_v4(embedded_in_low_32([s[1], s[2]]));
        return if embedded.is_public() {
            AddressClass::Public
        } else {
            embedded
        };
    }
    match s[0] {
        0xff00..=0xffff => AddressClass::Multicast,
        0xfe80..=0xfebf => AddressClass::LinkLocal,
        0xfec0..=0xfeff => AddressClass::Private,
        0xfc00..=0xfdff => AddressClass::UniqueLocal,
        _ => AddressClass::Public,
    }
}

#[cfg(test)]
mod tests;

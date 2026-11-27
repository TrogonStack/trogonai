//! Resolve once, vet every candidate against the SSRF floor, dial only the
//! vetted address.
//!
//! A name the policy allows is attacker-influenced data: whoever controls its
//! DNS records decides what it resolves to. The floor refuses loopback,
//! unspecified, metadata, link-local, multicast, broadcast, this host's own
//! interface addresses and (multi-tenant default) RFC 1918, CGNAT and ULA
//! space, unless the lease policy carves the address out with an explicit
//! `allow_ranges` entry. IPv6 forms that carry an IPv4 address are judged as
//! that address too, and carry no carve-out of their own.

use crate::host_kind::{HostClassification, HostKind};
use std::io;
use std::net::{IpAddr, SocketAddr};
#[cfg(any(unix, windows))]
use std::net::{Ipv4Addr, Ipv6Addr};
use std::sync::Arc;

use async_trait::async_trait;
use ipnet::IpNet;
use trogon_sandbox_contract::{AddressDenialClass, EgressViolationReason, IpRange, NetworkPolicy};
use trogon_sandbox_contract::{Host, Port};
use trogon_sandbox_protocol::{Sentinel, is_loopback_name};
#[cfg(windows)]
use windows::Win32::Foundation::{ERROR_BUFFER_OVERFLOW, ERROR_SUCCESS};
#[cfg(windows)]
use windows::Win32::NetworkManagement::IpHelper::{
    GAA_FLAG_SKIP_ANYCAST, GAA_FLAG_SKIP_DNS_SERVER, GAA_FLAG_SKIP_MULTICAST, GetAdaptersAddresses,
    IP_ADAPTER_ADDRESSES_LH,
};
#[cfg(windows)]
use windows::Win32::Networking::WinSock::{AF_INET, AF_INET6, AF_UNSPEC, SOCKADDR_IN, SOCKADDR_IN6};

use crate::binding_cache::BindingCache;
use crate::fold_mapped::fold_mapped;

mod constants;
use constants::FLOOR_NETS;

/// An address that passed the SSRF floor. Only [`SsrfFloor::vet`] mints one,
/// so a dialer taking this type cannot be handed an unchecked address. The
/// one other kind is `Self::unresolved`, which stands for a destination that
/// a MITM socket or parent proxy resolves by name and which no dialer opens.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash)]
pub struct VettedAddr {
    ip: IpAddr,
    origin: Origin,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash)]
enum Origin {
    Vetted,
    NamedLiteral,
    NamedHost,
}

impl VettedAddr {
    fn vetted(ip: IpAddr) -> Self {
        Self {
            ip,
            origin: Origin::Vetted,
        }
    }

    /// A destination that was not resolved here, as when the route carries
    /// the name to a MITM socket or parent proxy. `ip` is the literal the
    /// client named, if any. It is never dialed.
    pub(crate) fn unresolved(ip: Option<IpAddr>) -> Self {
        match ip {
            Some(ip) => Self {
                ip,
                origin: Origin::NamedLiteral,
            },
            None => Self {
                ip: IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED),
                origin: Origin::NamedHost,
            },
        }
    }

    pub fn ip(self) -> IpAddr {
        self.ip
    }

    pub fn with_port(self, port: u16) -> VettedEndpoint {
        VettedEndpoint { addr: self, port }
    }

    pub(crate) fn is_vetted(self) -> bool {
        self.origin == Origin::Vetted
    }

    /// The address to report for this destination: none for a name that was
    /// not resolved here, as no address is known to report.
    pub(crate) fn reported_ip(self) -> Option<IpAddr> {
        (self.origin != Origin::NamedHost).then_some(self.ip)
    }

    #[cfg(test)]
    pub(crate) fn assume_vetted(ip: IpAddr) -> Self {
        Self::vetted(ip)
    }
}

/// A vetted address and the port to dial on it.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash)]
pub struct VettedEndpoint {
    addr: VettedAddr,
    port: u16,
}

impl VettedEndpoint {
    pub fn addr(self) -> VettedAddr {
        self.addr
    }

    pub fn port(self) -> u16 {
        self.port
    }

    pub fn socket_addr(self) -> SocketAddr {
        SocketAddr::new(self.addr.ip, self.port)
    }
}

/// The `allow_ranges` of a lease policy: the only way to carve an exception
/// out of the floor. An entry is a CIDR range, or an address and port
/// (`10.0.0.5:8080`, `[fc00::1]:8080`) that carves the exception for that
/// destination port only.
#[derive(Debug, Clone, Default, Eq, PartialEq)]
pub struct AllowRanges(Vec<(IpNet, Option<u16>)>);

impl AllowRanges {
    /// Every range the policy allows, each with the one port it is scoped to, if any.
    pub fn from_policy(policy: &NetworkPolicy) -> Self {
        match policy {
            NetworkPolicy::UserDefined { allow_ranges, .. } => Self(
                allow_ranges
                    .iter()
                    .filter_map(|range| Self::entry(range.cidr()).map(|net| (net, range.port().map(|port| port.get()))))
                    .collect(),
            ),
            NetworkPolicy::AllowAll | NetworkPolicy::DenyAll => Self::default(),
        }
    }

    /// The exceptions that apply to connections to `port`.
    pub fn for_port(&self, port: u16) -> Self {
        Self(
            self.0
                .iter()
                .filter(|(_, only)| only.is_none_or(|only| only == port))
                .copied()
                .collect(),
        )
    }

    /// The ranges on every port, with IPv4-mapped IPv6 ranges folded into IPv4 and ranges that fold to nothing dropped.
    pub fn from_ranges(ranges: &[IpRange]) -> Self {
        Self(ranges.iter().filter_map(Self::entry).map(|net| (net, None)).collect())
    }

    fn entry(range: &IpRange) -> Option<IpNet> {
        IpNet::new(range.address(), range.prefix_len())
            .ok()
            .and_then(fold_mapped_net)
    }

    fn contains(&self, ip: IpAddr) -> bool {
        self.0.iter().any(|(net, _)| net.contains(&ip))
    }

    fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

#[derive(Debug, thiserror::Error, Eq, PartialEq)]
#[error("not an IP address: {0:?}")]
pub struct InvalidAddressError(String);

/// Parses a resolver answer the way the floor judges it: a zone id
/// (`fe80::1%eth0`) is dropped, since it selects an interface and never
/// changes which address is meant.
pub fn parse_address(text: &str) -> Result<IpAddr, InvalidAddressError> {
    let unzoned = text.split_once('%').map_or(text, |(address, _)| address);
    unzoned.parse().map_err(|_| InvalidAddressError(text.to_string()))
}

/// Unicast addresses currently assigned to this host's interfaces. A service
/// bound to `0.0.0.0` answers on them exactly as on loopback.
pub trait LocalInterfaces: Send + Sync {
    fn addresses(&self) -> io::Result<Vec<IpAddr>>;
}

#[derive(Debug, Default, Clone, Copy)]
pub struct SystemInterfaces;

impl LocalInterfaces for SystemInterfaces {
    #[cfg(unix)]
    fn addresses(&self) -> io::Result<Vec<IpAddr>> {
        let mut head: *mut libc::ifaddrs = std::ptr::null_mut();
        (unsafe { libc::getifaddrs(&mut head) } == 0)
            .then_some(())
            .ok_or_else(io::Error::last_os_error)?;
        let mut addresses = Vec::new();
        let mut cursor = head;
        while !cursor.is_null() {
            let entry = unsafe { &*cursor };
            for sockaddr in unsafe { entry.ifa_addr.as_ref() }.into_iter() {
                match i32::from(sockaddr.sa_family) {
                    libc::AF_INET => {
                        let sin = unsafe { &*(entry.ifa_addr as *const libc::sockaddr_in) };
                        addresses.push(IpAddr::V4(Ipv4Addr::from(u32::from_be(sin.sin_addr.s_addr))));
                    }
                    libc::AF_INET6 => {
                        let sin6 = unsafe { &*(entry.ifa_addr as *const libc::sockaddr_in6) };
                        addresses.push(IpAddr::V6(Ipv6Addr::from(sin6.sin6_addr.s6_addr)));
                    }
                    _ => {}
                }
            }
            cursor = entry.ifa_next;
        }
        unsafe { libc::freeifaddrs(head) };
        Ok(addresses)
    }

    #[cfg(windows)]
    fn addresses(&self) -> io::Result<Vec<IpAddr>> {
        let flags = GAA_FLAG_SKIP_ANYCAST | GAA_FLAG_SKIP_MULTICAST | GAA_FLAG_SKIP_DNS_SERVER;
        let mut len: u32 = 16 * 1024;
        loop {
            let mut buffer = vec![0u64; (len as usize).div_ceil(8)];
            let head = buffer.as_mut_ptr().cast::<IP_ADAPTER_ADDRESSES_LH>();
            // SAFETY: `head` points at `len` writable bytes, aligned for the adapter list.
            let status = unsafe { GetAdaptersAddresses(u32::from(AF_UNSPEC.0), flags, None, Some(head), &mut len) };
            if status == ERROR_BUFFER_OVERFLOW.0 {
                continue;
            }
            if status != ERROR_SUCCESS.0 {
                return Err(io::Error::from_raw_os_error(status as i32));
            }
            let mut addresses = Vec::new();
            let mut adapter = head.cast_const();
            while !adapter.is_null() {
                // SAFETY: the list lives in `buffer`, which outlives this walk.
                let entry = unsafe { &*adapter };
                let mut unicast = entry.FirstUnicastAddress;
                while !unicast.is_null() {
                    let address = unsafe { &*unicast };
                    let sockaddr = address.Address.lpSockaddr;
                    if !sockaddr.is_null() {
                        match unsafe { (*sockaddr).sa_family } {
                            family if family == AF_INET => {
                                let sin = unsafe { &*sockaddr.cast::<SOCKADDR_IN>() };
                                let octets = unsafe { sin.sin_addr.S_un.S_addr }.to_ne_bytes();
                                addresses.push(IpAddr::V4(Ipv4Addr::from(octets)));
                            }
                            family if family == AF_INET6 => {
                                let sin6 = unsafe { &*sockaddr.cast::<SOCKADDR_IN6>() };
                                let octets = unsafe { sin6.sin6_addr.u.Byte };
                                addresses.push(IpAddr::V6(Ipv6Addr::from(octets)));
                            }
                            _ => {}
                        }
                    }
                    unicast = address.Next;
                }
                adapter = entry.Next;
            }
            return Ok(addresses);
        }
    }

    #[cfg(not(any(unix, windows)))]
    fn addresses(&self) -> io::Result<Vec<IpAddr>> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "interface enumeration is not supported on this platform",
        ))
    }
}

fn floor_class(ip: IpAddr, private_use_allowed: bool) -> Option<AddressDenialClass> {
    FLOOR_NETS
        .iter()
        .filter(|(class, _)| !(private_use_allowed && *class == AddressDenialClass::PrivateUse))
        .filter(|(_, net)| net.contains(&ip))
        .map(|(class, _)| *class)
        .min()
}

/// An IPv4-mapped IPv6 range is the IPv4 range it spells; one wider than the
/// mapped prefix would cover unrelated IPv6 space and is dropped.
fn fold_mapped_net(net: IpNet) -> Option<IpNet> {
    let IpNet::V6(v6) = net else { return Some(net) };
    match v6.addr().to_ipv4_mapped() {
        Some(v4) => v6
            .prefix_len()
            .checked_sub(96)
            .and_then(|prefix| ipnet::Ipv4Net::new(v4, prefix).ok())
            .map(IpNet::V4),
        None => Some(net),
    }
}

/// The IPv4 address an IPv6 address carries in a form a network delivers to
/// that IPv4 destination: IPv4-compatible (`::/96`), IPv4-translated
/// (`::ffff:0:0:0/96`), NAT64 well-known (`64:ff9b::/96`) and 6to4
/// (`2002::/16`). Local-use and network-specific NAT64 prefixes cannot be
/// told from the address and are not decoded.
pub(crate) fn embedded_ipv4(ip: IpAddr) -> Option<Ipv4Addr> {
    let IpAddr::V6(v6) = ip else { return None };
    let s = v6.segments();
    let low = Ipv4Addr::new((s[6] >> 8) as u8, s[6] as u8, (s[7] >> 8) as u8, s[7] as u8);
    let zero = |range: std::ops::Range<usize>| s[range].iter().all(|segment| *segment == 0);
    if s[0] == 0x2002 {
        return Some(Ipv4Addr::new(
            (s[1] >> 8) as u8,
            s[1] as u8,
            (s[2] >> 8) as u8,
            s[2] as u8,
        ));
    }
    let nat64 = s[0] == 0x64 && s[1] == 0xff9b && zero(2..6);
    let compatible = zero(0..6);
    let translated = zero(0..4) && s[4] == 0xffff && s[5] == 0;
    (nat64 || compatible || translated).then_some(low)
}

/// The SSRF floor.
pub struct SsrfFloor {
    interfaces: Arc<dyn LocalInterfaces>,
    #[cfg(test)]
    loopback_exception: AllowRanges,
}

impl SsrfFloor {
    pub fn new(interfaces: Arc<dyn LocalInterfaces>) -> Self {
        Self {
            interfaces,
            #[cfg(test)]
            loopback_exception: AllowRanges::default(),
        }
    }

    #[cfg(test)]
    pub(crate) fn with_loopback_exception(mut self) -> Self {
        self.loopback_exception = AllowRanges("127.0.0.0/8".parse().into_iter().map(|net| (net, None)).collect());
        self
    }

    pub fn system() -> Self {
        Self::new(Arc::new(SystemInterfaces))
    }

    /// Judges `ip`, reading this host's interface addresses now. An address
    /// inside `exceptions` passes. Otherwise it is refused when it, or the
    /// IPv4 address it carries, falls in the floor or on a local interface.
    /// When interfaces cannot be enumerated the address is refused.
    pub fn vet(&self, ip: IpAddr, exceptions: &AllowRanges) -> Result<VettedAddr, AddressDenialClass> {
        self.judge(ip, exceptions, false)
    }

    fn judge(
        &self,
        ip: IpAddr,
        exceptions: &AllowRanges,
        private_use_allowed: bool,
    ) -> Result<VettedAddr, AddressDenialClass> {
        let addr = fold_mapped(ip);
        #[cfg(test)]
        let permitted = exceptions.contains(addr) || self.loopback_exception.contains(addr);
        #[cfg(not(test))]
        let permitted = exceptions.contains(addr);
        if permitted {
            return Ok(VettedAddr::vetted(addr));
        }
        let carried = embedded_ipv4(addr).map(IpAddr::V4);
        let candidates = std::iter::once(addr).chain(carried);

        let by_range = candidates
            .clone()
            .filter_map(|candidate| floor_class(candidate, private_use_allowed))
            .min();
        if let Some(class) = by_range {
            return Err(class);
        }

        let local = self
            .interfaces
            .addresses()
            .map_err(|_| AddressDenialClass::OwnInterface)?;
        let on_local_interface = candidates
            .map(fold_mapped)
            .any(|candidate| local.iter().copied().map(fold_mapped).any(|l| l == candidate));
        if on_local_interface {
            return Err(AddressDenialClass::OwnInterface);
        }
        Ok(VettedAddr::vetted(addr))
    }
}

/// Name resolution, separated from vetting so the floor can be exercised
/// without DNS.
#[async_trait]
pub trait NameLookup: Send + Sync {
    async fn lookup(&self, name: &str, port: Port) -> io::Result<Vec<IpAddr>>;
}

#[derive(Debug, Default, Clone, Copy)]
pub struct SystemLookup;

#[async_trait]
impl NameLookup for SystemLookup {
    async fn lookup(&self, name: &str, port: Port) -> io::Result<Vec<IpAddr>> {
        Ok(tokio::net::lookup_host((name, port.get()))
            .await?
            .map(|addr| addr.ip())
            .collect())
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ResolveError {
    #[error("address {addr} is refused")]
    Denied {
        class: AddressDenialClass,
        addr: IpAddr,
        classes: Vec<AddressDenialClass>,
        refused: Vec<IpAddr>,
    },
    #[error("name did not resolve: {0}")]
    Unresolved(io::Error),
    /// The name carries a secret sentinel, so it was never sent to DNS: the
    /// query alone would carry the sentinel to whoever answers for the zone.
    #[error("name carries a secret sentinel and is never resolved")]
    CarriesSentinel,
}

/// Resolves a destination once and returns the one vetted address the caller
/// must dial.
pub struct VettingResolver {
    lookup: Arc<dyn NameLookup>,
    floor: SsrfFloor,
    loopback_names: bool,
    private_ranges_by_name: bool,
    listed: AllowRanges,
    bindings: Arc<BindingCache>,
}

impl VettingResolver {
    pub fn new(lookup: Arc<dyn NameLookup>, floor: SsrfFloor) -> Self {
        Self {
            lookup,
            floor,
            loopback_names: false,
            private_ranges_by_name: false,
            listed: AllowRanges::default(),
            bindings: Arc::new(BindingCache::default()),
        }
    }

    /// Keeps each (name, address) pair this resolver observes for `ttl`
    /// instead of the default.
    pub fn with_binding_ttl(mut self, ttl: std::time::Duration) -> Self {
        self.bindings = Arc::new(BindingCache::new(ttl));
        self
    }

    /// The pairs this resolver's own lookups have produced, the only source
    /// from which a name is attributed to an address.
    pub fn bindings(&self) -> &BindingCache {
        &self.bindings
    }

    /// Lets a name resolve into the private-use ranges (RFC 1918, CGNAT,
    /// unique local). An IP literal is still refused there, and every other
    /// class of the floor still applies to names.
    pub fn with_private_ranges_by_name(mut self) -> Self {
        self.private_ranges_by_name = true;
        self
    }

    /// Refuses a name whose answer falls in `listed`, unless an allow range
    /// carves the answer out. An IP literal is never judged against it.
    pub fn with_listed_addresses(mut self, listed: AllowRanges) -> Self {
        self.listed = listed;
        self
    }

    /// Lets `localhost` and names under `.localhost` resolve to loopback and
    /// to nothing else, except an address an allow range carves out.
    pub fn with_loopback_names(mut self) -> Self {
        self.loopback_names = true;
        self
    }

    pub fn system() -> Self {
        Self::new(Arc::new(SystemLookup), SsrfFloor::system())
    }

    pub fn floor(&self) -> &SsrfFloor {
        &self.floor
    }

    /// An IP literal is vetted as is. A name is looked up once; refused
    /// answers are dropped and the first surviving one is returned, so the
    /// address that passed is the address dialed. When every answer is
    /// refused, the highest priority class is reported.
    pub async fn resolve(&self, host: &Host, port: Port, exceptions: &AllowRanges) -> Result<VettedAddr, ResolveError> {
        let mut all = self.resolve_all(host, port, exceptions).await?;
        Ok(all.remove(0))
    }

    /// Every answer that survives vetting, in the order the lookup gave them,
    /// never empty. The caller may dial them in that order.
    pub async fn resolve_all(
        &self,
        host: &Host,
        port: Port,
        exceptions: &AllowRanges,
    ) -> Result<Vec<VettedAddr>, ResolveError> {
        let candidates = match host.kind() {
            HostKind::Ip(ip) => vec![ip],
            HostKind::Domain(name) if carries_sentinel(name.as_str()) => return Err(ResolveError::CarriesSentinel),
            HostKind::Domain(name) => self
                .lookup
                .lookup(name.as_str(), port)
                .await
                .map_err(ResolveError::Unresolved)?,
        };
        for candidate in &candidates {
            self.bindings.record(host, *candidate);
        }

        let loopback_only =
            self.loopback_names && matches!(host.kind(), HostKind::Domain(name) if is_loopback_name(name.as_str()));
        let mut refused: Option<(AddressDenialClass, IpAddr)> = None;
        let mut refused_classes: Vec<AddressDenialClass> = Vec::new();
        let mut refused_addrs: Vec<IpAddr> = Vec::new();
        let mut vetted_all = Vec::new();
        let listed = self.listed.for_port(port.get());
        for candidate in candidates {
            let judged = if loopback_only {
                self.vet_loopback_name(candidate, exceptions)
            } else if self.private_ranges_by_name && matches!(host.kind(), HostKind::Domain(_)) {
                self.floor.judge(candidate, exceptions, true)
            } else {
                self.floor.vet(candidate, exceptions)
            };
            let judged = self.apply_listed(host, &listed, exceptions, candidate, judged);
            match judged {
                Ok(vetted) => {
                    if !vetted_all.contains(&vetted) {
                        vetted_all.push(vetted);
                    }
                }
                Err(class) => {
                    refused_addrs.push(candidate);
                    if !refused_classes.contains(&class) {
                        refused_classes.push(class);
                    }
                    if refused.is_none_or(|(best, _)| class < best) {
                        refused = Some((class, candidate));
                    }
                }
            }
        }
        if !vetted_all.is_empty() {
            return Ok(vetted_all);
        }
        match refused {
            Some((class, addr)) => Err(ResolveError::Denied {
                class,
                addr,
                classes: refused_classes,
                refused: refused_addrs,
            }),
            None => Err(ResolveError::Unresolved(io::Error::new(
                io::ErrorKind::NotFound,
                "no addresses",
            ))),
        }
    }
}

impl VettingResolver {
    fn apply_listed(
        &self,
        host: &Host,
        listed: &AllowRanges,
        exceptions: &AllowRanges,
        candidate: IpAddr,
        judged: Result<VettedAddr, AddressDenialClass>,
    ) -> Result<VettedAddr, AddressDenialClass> {
        if listed.is_empty() || !matches!(host.kind(), HostKind::Domain(_)) {
            return judged;
        }
        let addr = fold_mapped(candidate);
        if exceptions.contains(addr) {
            return judged;
        }
        let carried = embedded_ipv4(addr).map(IpAddr::V4);
        let is_listed = std::iter::once(addr).chain(carried).any(|ip| listed.contains(ip));
        match judged {
            Ok(_) | Err(AddressDenialClass::OwnInterface) if is_listed => Err(AddressDenialClass::Listed),
            other => other,
        }
    }

    fn vet_loopback_name(&self, candidate: IpAddr, exceptions: &AllowRanges) -> Result<VettedAddr, AddressDenialClass> {
        let addr = fold_mapped(candidate);
        if addr.is_loopback() || exceptions.contains(addr) {
            return Ok(VettedAddr::vetted(addr));
        }
        Err(AddressDenialClass::NonLoopback)
    }
}

/// Why a name a client presented was not accepted as naming the address the
/// connection was redirected from.
#[derive(Debug)]
pub enum NameBindingError {
    Unresolved(io::Error),
    NotAnAnswer,
    /// The name carries a secret sentinel and was never looked up.
    CarriesSentinel,
}

impl NameBindingError {
    /// The reason a connection is refused when the name its TLS handshake
    /// presented fails this check.
    pub(crate) fn sni_violation(&self) -> EgressViolationReason {
        match self {
            Self::NotAnAnswer => EgressViolationReason::SniDestinationMismatch,
            Self::Unresolved(_) => EgressViolationReason::SniResolutionFailed,
            Self::CarriesSentinel => EgressViolationReason::PolicyDenied,
        }
    }
}

impl VettingResolver {
    /// Confirms that `name` resolves, through the same lookup that vets
    /// dialed names, to `original`. Both sides are folded to their IPv4 form
    /// when mapped, so `::ffff:a.b.c.d` and `a.b.c.d` agree. The proxy's
    /// answer set is the only one consulted: what the client resolved is
    /// unknowable here, so the address the client reached must be among what
    /// the proxy resolves now. A name that does not resolve confirms nothing.
    /// The answers are not run through the floor; the dial vets `original`.
    pub async fn confirm_binding(&self, name: &Host, port: Port, original: IpAddr) -> Result<(), NameBindingError> {
        let answers = match name.kind() {
            HostKind::Ip(ip) => vec![ip],
            HostKind::Domain(domain) if carries_sentinel(domain.as_str()) => {
                return Err(NameBindingError::CarriesSentinel);
            }
            HostKind::Domain(domain) => self
                .lookup
                .lookup(domain.as_str(), port)
                .await
                .map_err(NameBindingError::Unresolved)?,
        };
        if let HostKind::Domain(_) = name.kind() {
            for answer in &answers {
                self.bindings.record(name, *answer);
            }
        }
        let original = fold_mapped(original);
        let bound = match name.kind() {
            HostKind::Domain(_) => self.bindings.is_bound(name, original),
            HostKind::Ip(_) => answers.into_iter().map(fold_mapped).any(|a| a == original),
        };
        if bound {
            Ok(())
        } else {
            Err(NameBindingError::NotAnAnswer)
        }
    }
}

/// Whether `name` must never be resolved because it carries a secret
/// sentinel. Every lookup this resolver makes passes through it.
pub fn carries_sentinel(name: &str) -> bool {
    Sentinel::appears_in(name)
}

#[cfg(test)]
mod tests;

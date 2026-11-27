use std::net::{IpAddr, Ipv4Addr};

use trogon_sandbox_contract::{DomainName, Host};

/// A destination host seen as either a registered domain name or an IP literal.
pub enum HostKind<'a> {
    Domain(&'a DomainName),
    Ip(IpAddr),
}

/// Classifies a [`Host`] for call sites that treat names and addresses differently.
pub trait HostClassification {
    /// Returns the domain name or the IP address the host holds.
    fn kind(&self) -> HostKind<'_>;
}

impl HostClassification for Host {
    fn kind(&self) -> HostKind<'_> {
        match self.as_domain() {
            Some(name) => HostKind::Domain(name),
            None => HostKind::Ip(self.as_ip().unwrap_or(IpAddr::V4(Ipv4Addr::UNSPECIFIED))),
        }
    }
}

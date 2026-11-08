use std::fmt;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::str::FromStr;

use crate::host_name::{HostName, HostNameError};

/// The host half of a destination: an exact name or an exact IP literal.
///
/// IPv4-mapped IPv6 literals are folded to their IPv4 form so one address has
/// one spelling, both when a permit is compared and when it is dialed.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum DestinationHost {
    Name(HostName),
    Ip(IpAddr),
}

impl DestinationHost {
    /// Parses the host as it appears in `host:port`: a bare name, a dotted
    /// IPv4 address, or an IPv6 address in brackets.
    pub fn parse(raw: &str) -> Result<Self, DestinationHostError> {
        if let Some(inner) = raw.strip_prefix('[') {
            let inner = inner.strip_suffix(']').ok_or(DestinationHostError::UnclosedBracket)?;
            let address = Ipv6Addr::from_str(inner).map_err(|_| DestinationHostError::InvalidIpv6)?;
            return Ok(Self::Ip(IpAddr::V6(address).to_canonical()));
        }
        if let Ok(address) = Ipv4Addr::from_str(raw) {
            return Ok(Self::Ip(IpAddr::V4(address)));
        }
        HostName::new(raw).map(Self::Name).map_err(DestinationHostError::Name)
    }
}

impl fmt::Display for DestinationHost {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Name(name) => name.fmt(f),
            Self::Ip(IpAddr::V4(address)) => address.fmt(f),
            Self::Ip(IpAddr::V6(address)) => write!(f, "[{address}]"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DestinationHostError {
    #[error("bracketed host is missing its closing bracket")]
    UnclosedBracket,
    #[error("bracketed host is not an IPv6 address")]
    InvalidIpv6,
    #[error("invalid host name")]
    Name(#[source] HostNameError),
}

#[cfg(test)]
mod tests;

use std::net::IpAddr;

use crate::NoProxyCidrError;

/// An address range of a `NO_PROXY` list: a network address and a prefix length.
///
/// An IPv4-mapped IPv6 network with a prefix of at least 96 bits is stored as the IPv4 network with 96 fewer prefix
/// bits, and a mapped address without a prefix as a single IPv4 address. An entry without a prefix covers one
/// address.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash)]
pub struct NoProxyCidr {
    network: IpAddr,
    prefix: u8,
}

impl NoProxyCidr {
    pub fn new(entry: &str) -> Result<Self, NoProxyCidrError> {
        let invalid = || NoProxyCidrError::Invalid(entry.to_string());
        let (address, prefix) = match entry.split_once('/') {
            Some((address, prefix)) => (address, Some(prefix.parse::<u8>().map_err(|_| invalid())?)),
            None => (entry, None),
        };
        let address = address.parse::<IpAddr>().map_err(|_| invalid())?;
        let (network, prefix) = normalize(address, prefix);
        let max = max_prefix(network);
        let prefix = prefix.unwrap_or(max);
        if prefix > max {
            return Err(NoProxyCidrError::PrefixTooLong {
                input: entry.to_string(),
                max,
            });
        }
        Ok(Self { network, prefix })
    }

    pub fn network(&self) -> IpAddr {
        self.network
    }

    pub fn prefix(&self) -> u8 {
        self.prefix
    }
}

impl From<IpAddr> for NoProxyCidr {
    fn from(address: IpAddr) -> Self {
        let (network, _) = normalize(address, None);
        Self {
            network,
            prefix: max_prefix(network),
        }
    }
}

fn normalize(address: IpAddr, prefix: Option<u8>) -> (IpAddr, Option<u8>) {
    match address {
        IpAddr::V6(v6) => match (v6.to_ipv4_mapped(), prefix) {
            (Some(v4), Some(prefix)) if prefix >= 96 => (IpAddr::V4(v4), Some(prefix - 96)),
            (Some(v4), None) => (IpAddr::V4(v4), None),
            _ => (IpAddr::V6(v6), prefix),
        },
        v4 => (v4, prefix),
    }
}

fn max_prefix(network: IpAddr) -> u8 {
    if network.is_ipv4() { 32 } else { 128 }
}

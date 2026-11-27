use std::fmt;
use std::net::{IpAddr, Ipv6Addr};
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::{DomainName, HostError};

/// A destination host in canonical form, so string comparison agrees with what the OS resolver will dial.
///
/// Parsing follows WHATWG host rules: IDNA, lowercase, `inet_aton` shorthand such as `127.1`, and IPv6 compression.
/// A trailing dot is removed and an IPv4-mapped IPv6 address is spelled as IPv4. Zone ids, percent escapes, control
/// characters, and an overlong or malformed DNS name are rejected. It serializes as a JSON string, with an IPv6
/// address in brackets.
#[derive(Debug, Clone, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Host(Kind);

#[derive(Debug, Clone, Eq, PartialEq, Hash)]
enum Kind {
    Domain(DomainName),
    Ip(IpAddr),
}

impl Host {
    pub fn new(input: &str) -> Result<Self, HostError> {
        if input.is_empty() {
            return Err(HostError::Empty);
        }
        let invalid = || HostError::Invalid(input.to_string());
        if input.contains('%') {
            return Err(invalid());
        }
        if input.contains(':') && !input.starts_with('[') {
            let ip = input.parse::<Ipv6Addr>().map_err(|source| HostError::InvalidIpv6 {
                input: input.to_string(),
                source,
            })?;
            return Ok(Self::from(IpAddr::V6(ip)));
        }
        let parsed = url::Host::parse(input).map_err(|source| HostError::Unparsable {
            input: input.to_string(),
            source,
        })?;
        match parsed {
            url::Host::Ipv4(ip) => Ok(Self(Kind::Ip(IpAddr::V4(ip)))),
            url::Host::Ipv6(ip) => Ok(Self::from(IpAddr::V6(ip))),
            url::Host::Domain(name) => {
                let name = name.strip_suffix('.').unwrap_or(&name);
                DomainName::new(name)
                    .map(|name| Self(Kind::Domain(name)))
                    .ok_or_else(invalid)
            }
        }
    }

    /// The domain name, when the host is one.
    pub fn as_domain(&self) -> Option<&DomainName> {
        match &self.0 {
            Kind::Domain(name) => Some(name),
            Kind::Ip(_) => None,
        }
    }

    /// The IP address in canonical form, when the host is one.
    pub fn as_ip(&self) -> Option<IpAddr> {
        match &self.0 {
            Kind::Domain(_) => None,
            Kind::Ip(ip) => Some(*ip),
        }
    }
}

impl From<IpAddr> for Host {
    fn from(ip: IpAddr) -> Self {
        match ip {
            IpAddr::V6(v6) => Self(Kind::Ip(v6.to_ipv4_mapped().map_or(ip, IpAddr::V4))),
            v4 => Self(Kind::Ip(v4)),
        }
    }
}

impl TryFrom<String> for Host {
    type Error = HostError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(&value)
    }
}

impl FromStr for Host {
    type Err = HostError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::new(value)
    }
}

impl From<Host> for String {
    fn from(host: Host) -> Self {
        host.to_string()
    }
}

impl fmt::Display for Host {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.0 {
            Kind::Domain(name) => name.fmt(formatter),
            Kind::Ip(IpAddr::V6(ip)) => write!(formatter, "[{ip}]"),
            Kind::Ip(ip) => ip.fmt(formatter),
        }
    }
}

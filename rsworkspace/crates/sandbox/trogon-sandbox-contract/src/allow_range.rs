use std::fmt;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::{AllowRangeError, IpRange, Port};

/// One `allow_ranges` entry of a user defined network policy: a CIDR range (`10.0.0.0/8`), a single address
/// (`10.0.0.6`, `fc00::1`), or a socket address (`10.0.0.5:8080`, `[fc00::1]:8080`) whose port scopes the exception
/// to that destination port only.
///
/// The CIDR form follows [`IpRange`]. A single address stands for its full-length range. The port of a socket address
/// is 1 to 65535 without a sign or leading zero, and an IPv6 address with a port is written in brackets. It
/// serializes as the JSON string it was read from.
#[derive(Debug, Clone, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct AllowRange {
    text: String,
    cidr: IpRange,
    port: Option<Port>,
}

impl AllowRange {
    pub fn new(value: impl Into<String>) -> Result<Self, AllowRangeError> {
        let text = value.into();
        match parse(&text) {
            Some((cidr, port)) => Ok(Self { text, cidr, port }),
            None => Err(AllowRangeError(text)),
        }
    }

    pub fn as_str(&self) -> &str {
        &self.text
    }

    /// The addresses the entry covers, as a CIDR range. A single address, with or without a port, is its
    /// full-length range.
    pub fn cidr(&self) -> &IpRange {
        &self.cidr
    }

    /// The only destination port the entry allows, or `None` when it allows every port.
    pub fn port(&self) -> Option<Port> {
        self.port
    }

    /// Whether the entry allows connections to `port`.
    pub fn allows_port(&self, port: u16) -> bool {
        self.port.is_none_or(|only| only.get() == port)
    }
}

fn parse(text: &str) -> Option<(IpRange, Option<Port>)> {
    if text.contains('/') {
        return IpRange::new(text).ok().map(|cidr| (cidr, None));
    }
    if let Ok(address) = text.parse::<IpAddr>() {
        return single(address).map(|cidr| (cidr, None));
    }
    let (host, port) = text.rsplit_once(':')?;
    let address = match host.strip_prefix('[').and_then(|inner| inner.strip_suffix(']')) {
        Some(inner) => IpAddr::V6(inner.parse::<Ipv6Addr>().ok()?),
        None => IpAddr::V4(host.parse::<Ipv4Addr>().ok()?),
    };
    let port = port.parse::<Port>().ok()?;
    single(address).map(|cidr| (cidr, Some(port)))
}

fn single(address: IpAddr) -> Option<IpRange> {
    let prefix_len = if address.is_ipv4() { 32 } else { 128 };
    IpRange::new(format!("{address}/{prefix_len}")).ok()
}

impl From<IpRange> for AllowRange {
    fn from(cidr: IpRange) -> Self {
        Self {
            text: cidr.as_str().to_owned(),
            cidr,
            port: None,
        }
    }
}

impl TryFrom<String> for AllowRange {
    type Error = AllowRangeError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl FromStr for AllowRange {
    type Err = AllowRangeError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::new(value)
    }
}

impl From<AllowRange> for String {
    fn from(value: AllowRange) -> Self {
        value.text
    }
}

impl fmt::Display for AllowRange {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.text)
    }
}

use std::fmt;
use std::net::IpAddr;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::IpRangeError;

/// An IP range in CIDR notation, such as `10.0.0.0/8` or `2001:db8::/32`.
///
/// Construction, parsing, and deserialization accept an IPv4 or IPv6 address, a `/`, and a prefix length of at most
/// 32 or 128 written without a sign or leading zero. Host bits may be set. It serializes as the JSON string it was
/// read from.
#[derive(Debug, Clone, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct IpRange {
    text: String,
    address: IpAddr,
    prefix_len: u8,
}

impl IpRange {
    pub fn new(value: impl Into<String>) -> Result<Self, IpRangeError> {
        let text = value.into();
        match parse(&text) {
            Some((address, prefix_len)) => Ok(Self {
                text,
                address,
                prefix_len,
            }),
            None => Err(IpRangeError(text)),
        }
    }

    pub fn as_str(&self) -> &str {
        &self.text
    }

    /// The address before the `/`.
    pub fn address(&self) -> IpAddr {
        self.address
    }

    /// The number of leading bits the range fixes.
    pub fn prefix_len(&self) -> u8 {
        self.prefix_len
    }
}

fn parse(text: &str) -> Option<(IpAddr, u8)> {
    let (address, prefix) = text.split_once('/')?;
    let address = address.parse::<IpAddr>().ok()?;
    let digits = !prefix.is_empty() && prefix.len() <= 3 && prefix.bytes().all(|byte| byte.is_ascii_digit());
    if !digits || (prefix.len() > 1 && prefix.starts_with('0')) {
        return None;
    }
    let prefix_len = prefix.parse::<u8>().ok()?;
    let max = if address.is_ipv4() { 32 } else { 128 };
    (prefix_len <= max).then_some((address, prefix_len))
}

impl TryFrom<String> for IpRange {
    type Error = IpRangeError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl FromStr for IpRange {
    type Err = IpRangeError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::new(value)
    }
}

impl From<IpRange> for String {
    fn from(value: IpRange) -> Self {
        value.text
    }
}

impl fmt::Display for IpRange {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.text)
    }
}

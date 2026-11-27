use std::fmt;
use std::num::ParseIntError;
use std::str::FromStr;

/// Unix uid the egress proxy runs as when the transparent redirect exempts it with `--uid-owner` instead of a
/// [`SocketMark`](crate::SocketMark).
///
/// It must differ from every uid the sandboxed workload can run as. It renders and parses as a decimal number.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash)]
pub struct ProxyUid(u32);

impl ProxyUid {
    pub const EGRESS_PROXY: Self = Self(65531);

    pub const fn new(uid: u32) -> Self {
        Self(uid)
    }

    pub const fn get(self) -> u32 {
        self.0
    }
}

impl fmt::Display for ProxyUid {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", self.0)
    }
}

impl FromStr for ProxyUid {
    type Err = ParseIntError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        value.parse().map(Self)
    }
}

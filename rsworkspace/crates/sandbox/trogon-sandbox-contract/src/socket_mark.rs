use std::fmt;

/// Netfilter mark the egress proxy sets (`SO_MARK`) on every socket it dials upstream.
///
/// The transparent redirect uses it to exempt the proxy's own traffic. It renders as hex, the form `iptables`
/// takes.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash)]
pub struct SocketMark(u32);

impl SocketMark {
    pub const EGRESS_PROXY: Self = Self(0x5352);

    pub const fn get(self) -> u32 {
        self.0
    }
}

impl fmt::Display for SocketMark {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{:#x}", self.0)
    }
}

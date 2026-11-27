use std::fmt;

/// The loopback ports the fence permits the sandbox account to reach, which
/// is where the lease proxy listens on Windows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ProxyPortRange {
    pub(crate) low: u16,
    pub(crate) high: u16,
}

impl ProxyPortRange {
    pub(crate) const fn reported(low: u16, high: u16) -> Self {
        Self { low, high }
    }

    pub fn low(&self) -> u16 {
        self.low
    }

    pub fn high(&self) -> u16 {
        self.high
    }

    pub fn contains(&self, port: u16) -> bool {
        (self.low..=self.high).contains(&port)
    }
}

impl fmt::Display for ProxyPortRange {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}-{}", self.low, self.high)
    }
}

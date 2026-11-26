use std::fmt;
use std::io;
use std::net::Ipv4Addr;
use std::str::FromStr;

use crate::constants::DEFAULT_PROXY_PORT_RANGE;
use crate::proxy_port_range_error::ProxyPortRangeError;

/// The widest range fence-helper accepts, in ports beyond the first.
const MAX_WIDTH: u16 = 64;

/// The loopback ports the fence permits the sandbox account to reach, which
/// is where the lease proxy listens on Windows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ProxyPortRange {
    low: u16,
    high: u16,
}

impl ProxyPortRange {
    pub fn new(low: u16, high: u16) -> Result<Self, ProxyPortRangeError> {
        if low == 0 {
            return Err(ProxyPortRangeError::ZeroLow);
        }
        if low > high {
            return Err(ProxyPortRangeError::Inverted { low, high });
        }
        if high - low > MAX_WIDTH {
            return Err(ProxyPortRangeError::TooWide {
                ports: u32::from(high - low) + 1,
                max: u32::from(MAX_WIDTH) + 1,
            });
        }
        Ok(Self { low, high })
    }

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

    /// Binds a loopback listener on the first free port of the range and
    /// returns it still bound, only a port in
    /// use moves on to the next one, any other failure is the result, and a
    /// port in `exclude` is skipped.
    pub fn listen(&self, exclude: &[u16]) -> io::Result<std::net::TcpListener> {
        self.listen_with(exclude, |port| std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, port)))
    }

    fn listen_with(
        &self,
        exclude: &[u16],
        mut bind: impl FnMut(u16) -> io::Result<std::net::TcpListener>,
    ) -> io::Result<std::net::TcpListener> {
        for port in (self.low..=self.high).filter(|port| !exclude.contains(port)) {
            match bind(port) {
                Ok(listener) => return Ok(listener),
                Err(error) if error.kind() == io::ErrorKind::AddrInUse => {}
                Err(error) => return Err(error),
            }
        }
        let excluded = exclude.iter().map(u16::to_string).collect::<Vec<_>>().join(",");
        Err(io::Error::new(
            io::ErrorKind::AddrInUse,
            format!(
                "No free port in range {}-{} (excluding {excluded})",
                self.low, self.high
            ),
        ))
    }
}

impl Default for ProxyPortRange {
    fn default() -> Self {
        DEFAULT_PROXY_PORT_RANGE
    }
}

impl fmt::Display for ProxyPortRange {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}-{}", self.low, self.high)
    }
}

impl FromStr for ProxyPortRange {
    type Err = ProxyPortRangeError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let (low, high) = s.split_once('-').ok_or(ProxyPortRangeError::MissingSeparator)?;
        let low: u16 = low
            .trim()
            .parse()
            .map_err(|_| ProxyPortRangeError::InvalidLow(low.to_string()))?;
        let high: u16 = high
            .trim()
            .parse()
            .map_err(|_| ProxyPortRangeError::InvalidHigh(high.to_string()))?;
        Self::new(low, high)
    }
}

#[cfg(test)]
mod tests;

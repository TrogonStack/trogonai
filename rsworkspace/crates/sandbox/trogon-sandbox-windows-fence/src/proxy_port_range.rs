use std::io;
use std::net::Ipv4Addr;
use std::str::FromStr;

use crate::constants::DEFAULT_PROXY_PORT_RANGE;
use crate::constants::MAX_WIDTH;
use crate::proxy_port_range_error::ProxyPortRangeError;
pub use crate::proxy_port_range_value::ProxyPortRange;

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

use std::fmt;
use std::num::NonZeroU16;
use std::str::FromStr;

use crate::destination_host::{DestinationHost, DestinationHostError};

/// An exact `host:port` the proxy can be asked to reach, and the unit a permit
/// is written in. The CONNECT request target and every policy entry parse
/// through this one type, so they cannot disagree about what a destination is.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Destination {
    host: DestinationHost,
    port: NonZeroU16,
}

impl Destination {
    pub fn new(host: DestinationHost, port: NonZeroU16) -> Self {
        Self { host, port }
    }

    pub fn host(&self) -> &DestinationHost {
        &self.host
    }

    pub fn port(&self) -> NonZeroU16 {
        self.port
    }
}

impl FromStr for Destination {
    type Err = DestinationError;

    fn from_str(raw: &str) -> Result<Self, Self::Err> {
        let (host, port) = raw.rsplit_once(':').ok_or(DestinationError::MissingPort)?;
        if port.is_empty() || !port.bytes().all(|b| b.is_ascii_digit()) {
            return Err(DestinationError::InvalidPort);
        }
        let port = port
            .parse::<u16>()
            .map_err(|_| DestinationError::InvalidPort)
            .and_then(|p| NonZeroU16::new(p).ok_or(DestinationError::ZeroPort))?;
        let host = DestinationHost::parse(host).map_err(DestinationError::Host)?;
        Ok(Self { host, port })
    }
}

impl fmt::Display for Destination {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.host, self.port)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DestinationError {
    #[error("destination must be host:port, the port is missing")]
    MissingPort,
    #[error("destination port must be decimal digits from 1 to 65535")]
    InvalidPort,
    #[error("destination port must not be 0")]
    ZeroPort,
    #[error("invalid destination host")]
    Host(#[source] DestinationHostError),
}

#[cfg(test)]
mod tests;

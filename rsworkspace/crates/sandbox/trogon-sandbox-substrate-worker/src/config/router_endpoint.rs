use std::fmt;
use std::str::FromStr;

use super::RouterEndpointError;

/// A `host:port` pair naming the substrate router's CONNECT-tunnel listener.
///
/// A bare `String` would let a malformed value surface as a connection failure deep inside
/// a lease instead of at startup, so parsing happens once, here.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct RouterEndpoint {
    host: String,
    port: u16,
}

impl RouterEndpoint {
    pub fn host(&self) -> &str {
        &self.host
    }

    pub fn port(&self) -> u16 {
        self.port
    }
}

impl fmt::Display for RouterEndpoint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.host, self.port)
    }
}

impl FromStr for RouterEndpoint {
    type Err = RouterEndpointError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let (host, port) = value.rsplit_once(':').ok_or_else(|| RouterEndpointError::MissingPort {
            value: value.to_string(),
        })?;
        if host.is_empty() {
            return Err(RouterEndpointError::EmptyHost {
                value: value.to_string(),
            });
        }
        let port = port.parse::<u16>().map_err(|source| RouterEndpointError::InvalidPort {
            value: value.to_string(),
            source,
        })?;
        Ok(Self {
            host: host.to_string(),
            port,
        })
    }
}

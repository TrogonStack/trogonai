use std::fmt;

use crate::egress_host_name::EgressHostName;
use crate::port::Port;

/// One exact destination: a host and a port, with no wildcard on either side.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct EgressDestination {
    host: EgressHostName,
    port: Port,
}

impl EgressDestination {
    /// Names a destination.
    pub fn new(host: EgressHostName, port: Port) -> Self {
        Self { host, port }
    }

    /// The host.
    pub fn host(&self) -> &EgressHostName {
        &self.host
    }

    /// The port.
    pub fn port(&self) -> Port {
        self.port
    }

    /// Returns whether this is exactly the requested destination.
    pub fn matches(&self, host: &EgressHostName, port: Port) -> bool {
        self.host == *host && self.port == port
    }
}

impl fmt::Display for EgressDestination {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.host, self.port)
    }
}

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

    /// Returns whether this is exactly the requested destination.
    pub fn matches(&self, host: &EgressHostName, port: Port) -> bool {
        self.host == *host && self.port == port
    }
}

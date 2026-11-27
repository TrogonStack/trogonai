use std::fmt;

use trogon_sandbox_contract::{Host, Port};

/// The destination a client asked to reach.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct ConnectTarget {
    host: Host,
    port: Port,
}

impl ConnectTarget {
    pub fn new(host: Host, port: Port) -> Self {
        Self { host, port }
    }

    pub fn host(&self) -> &Host {
        &self.host
    }

    pub fn port(&self) -> Port {
        self.port
    }
}

impl fmt::Display for ConnectTarget {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.host, self.port)
    }
}

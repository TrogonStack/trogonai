use crate::Port;

/// A proxy the caller already runs, used instead of starting one for the side it covers.
///
/// HTTP and SOCKS may be served on different ports. A side with no port of its own is served by the runtime's own
/// proxy.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub struct ExternalProxy {
    http: Option<Port>,
    socks: Option<Port>,
}

impl ExternalProxy {
    pub fn http_only(port: Port) -> Self {
        Self {
            http: Some(port),
            socks: None,
        }
    }

    pub fn socks_only(port: Port) -> Self {
        Self {
            http: None,
            socks: Some(port),
        }
    }

    pub fn both(http: Port, socks: Port) -> Self {
        Self {
            http: Some(http),
            socks: Some(socks),
        }
    }

    pub fn http_port(self) -> Option<Port> {
        self.http
    }

    pub fn socks_port(self) -> Option<Port> {
        self.socks
    }
}

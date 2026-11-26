use std::fmt;
use std::num::NonZeroU16;

use trogon_credential_domain::ReleaseHost;

/// Where the released value is about to be sent.
///
/// The release policy matches the host alone; the port travels with it so the
/// audit record names the exact endpoint.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReleaseDestination {
    host: ReleaseHost,
    port: NonZeroU16,
}

impl ReleaseDestination {
    pub fn new(host: ReleaseHost, port: NonZeroU16) -> Self {
        Self { host, port }
    }

    pub fn host(&self) -> &ReleaseHost {
        &self.host
    }

    pub fn port(&self) -> NonZeroU16 {
        self.port
    }
}

impl fmt::Display for ReleaseDestination {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.host, self.port)
    }
}

#[cfg(test)]
mod tests;

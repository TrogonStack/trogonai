use std::num::NonZeroU16;

use trogon_credential_domain::ReleaseHost;
use trogonai_proto::policy::egress_permissions::v1alpha1::EgressHost;

/// The exact host and port the released value is about to be sent to.
///
/// The secrets service checks a release against one concrete destination, so
/// the port is part of it: the same host on another port is another release.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ReleaseDestination {
    host: String,
    port: NonZeroU16,
}

impl ReleaseDestination {
    pub fn new(host: &ReleaseHost, port: NonZeroU16) -> Self {
        Self {
            host: host.as_str().to_owned(),
            port,
        }
    }

    pub fn host(&self) -> &str {
        &self.host
    }

    pub const fn port(&self) -> NonZeroU16 {
        self.port
    }

    pub(crate) fn to_wire(&self) -> EgressHost {
        EgressHost {
            host: self.host.clone(),
            port: u32::from(self.port.get()),
        }
    }
}

#[cfg(test)]
mod tests;

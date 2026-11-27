use crate::{DomainPattern, IpRange};

/// Destinations an operator relays without terminating TLS, even when a rule, a secret binding, or the
/// terminate-all switch would terminate them.
///
/// An exemption grants no egress: it applies only to a flow the policy already admitted. An exempt flow gets no HTTP
/// inspection and no credential injection.
#[derive(Debug, Clone, Default, Eq, PartialEq)]
pub struct TlsInterceptionExemptions {
    hostnames: Vec<DomainPattern>,
    addresses: Vec<IpRange>,
}

impl TlsInterceptionExemptions {
    pub fn new(
        hostnames: impl IntoIterator<Item = DomainPattern>,
        addresses: impl IntoIterator<Item = IpRange>,
    ) -> Self {
        Self {
            hostnames: hostnames.into_iter().collect(),
            addresses: addresses.into_iter().collect(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.hostnames.is_empty() && self.addresses.is_empty()
    }

    pub fn hostnames(&self) -> &[DomainPattern] {
        &self.hostnames
    }

    pub fn addresses(&self) -> &[IpRange] {
        &self.addresses
    }
}

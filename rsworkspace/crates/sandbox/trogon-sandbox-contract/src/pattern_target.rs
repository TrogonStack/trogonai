use crate::{DomainName, Host};

/// What a [`DomainPattern`](crate::DomainPattern) names, apart from its port.
#[derive(Debug, Clone, Eq, PartialEq, Hash)]
pub enum PatternTarget {
    /// Every destination. Only a [`DeniedDomain`](crate::DeniedDomain) may name it.
    Any,
    /// Exactly one host.
    Exact(Host),
    /// Every strict subdomain of a name, written `*.name`.
    Subdomains(DomainName),
}

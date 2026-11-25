use crate::NoProxyCidr;

/// One entry of a [`NoProxy`](crate::NoProxy) list.
#[derive(Debug, Clone, Eq, PartialEq, Hash)]
pub enum NoProxyEntry {
    /// `*`: every host skips the parent proxy.
    Wildcard,
    /// A lowercase host name. A leading dot, or `*.` read as one, marks subdomains.
    Suffix(String),
    /// An address range.
    Cidr(NoProxyCidr),
}

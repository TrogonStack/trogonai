use crate::DomainPattern;

/// Hosts the proxy never terminates TLS for: their connections stay opaque tunnels, still subject to the allow list.
#[derive(Debug, Clone, Default, Eq, PartialEq)]
pub struct TlsExclusions(Vec<DomainPattern>);

impl TlsExclusions {
    pub fn new(patterns: impl IntoIterator<Item = DomainPattern>) -> Self {
        Self(patterns.into_iter().collect())
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn as_slice(&self) -> &[DomainPattern] {
        &self.0
    }
}

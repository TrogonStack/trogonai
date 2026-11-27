use serde::{Deserialize, Serialize};

/// Whether the egress proxy carries a connection whose bytes it cannot read to an allowed destination.
///
/// Such a connection is neither TLS the proxy terminates nor HTTP it parses. `Allow` is the default and the
/// behavior of every policy written before this switch existed. `Refuse` closes such a connection and records a
/// violation, so every byte that leaves is one the proxy inspected; it stops protocols such as SSH through the
/// proxy.
#[derive(Debug, Clone, Copy, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OpaqueTunnelPolicy {
    #[default]
    Allow,
    Refuse,
}

impl OpaqueTunnelPolicy {
    pub fn is_allow(&self) -> bool {
        matches!(self, Self::Allow)
    }

    pub fn refuses(&self) -> bool {
        matches!(self, Self::Refuse)
    }
}

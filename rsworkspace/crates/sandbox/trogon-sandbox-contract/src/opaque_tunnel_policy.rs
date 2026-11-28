use serde::{Deserialize, Serialize};

/// Whether the egress proxy carries a connection whose bytes it cannot read to an allowed destination.
///
/// Such a connection is neither TLS the proxy terminates nor HTTP it parses: raw bytes after a `CONNECT`, a SOCKS5
/// tunnel, or TLS to a pass-through rule the proxy relays without terminating. `Refuse` is the default: it closes
/// such a connection and records a violation, so every byte that leaves is one the proxy inspected, and an allowed
/// host and port cannot be used as a pipe for data the proxy never saw. `Allow` is an explicit opt-in for untouched
/// TLS relays and protocols such as SSH or a database wire protocol through the proxy; a policy has to say so.
///
/// A policy that omits the field reads as `Refuse`, and a policy is always written with the field, so a reader never
/// has to infer the choice from its absence.
#[derive(Debug, Clone, Copy, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OpaqueTunnelPolicy {
    Allow,
    #[default]
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

use serde::{Deserialize, Serialize};

/// Whether the payload may look up the macOS trust agent service, which Go programs need to verify certificates against the proxy's CA. It is a way to exfiltrate data through the trust service.
///
/// It serializes as a JSON boolean, `true` for `Allowed`, so existing peers keep parsing it. An omitted value reads
/// as `Denied`.
#[derive(Debug, Clone, Copy, Default, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(from = "bool", into = "bool")]
pub enum TrustServiceAccess {
    #[default]
    Denied,
    Allowed,
}

impl TrustServiceAccess {
    pub const fn is_allowed(self) -> bool {
        matches!(self, Self::Allowed)
    }
}

impl From<bool> for TrustServiceAccess {
    fn from(flag: bool) -> Self {
        if flag { Self::Allowed } else { Self::Denied }
    }
}

impl From<TrustServiceAccess> for bool {
    fn from(value: TrustServiceAccess) -> Self {
        value.is_allowed()
    }
}

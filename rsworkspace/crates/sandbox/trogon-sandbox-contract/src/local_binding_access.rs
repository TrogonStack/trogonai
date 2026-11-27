use serde::{Deserialize, Serialize};

/// Whether the payload may bind and accept on any local port and connect to loopback on any port on macOS, which reaches every service listening on localhost.
///
/// It serializes as a JSON boolean, `true` for `Allowed`, so existing peers keep parsing it. An omitted value reads
/// as `Denied`.
#[derive(Debug, Clone, Copy, Default, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(from = "bool", into = "bool")]
pub enum LocalBindingAccess {
    #[default]
    Denied,
    Allowed,
}

impl LocalBindingAccess {
    pub const fn is_allowed(self) -> bool {
        matches!(self, Self::Allowed)
    }
}

impl From<bool> for LocalBindingAccess {
    fn from(flag: bool) -> Self {
        if flag { Self::Allowed } else { Self::Denied }
    }
}

impl From<LocalBindingAccess> for bool {
    fn from(value: LocalBindingAccess) -> Self {
        value.is_allowed()
    }
}

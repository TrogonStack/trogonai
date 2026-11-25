use serde::{Deserialize, Serialize};

/// Whether the payload may send Apple events and use the launch services they need, which lets it drive other applications on macOS.
///
/// It serializes as a JSON boolean, `true` for `Allowed`, so existing peers keep parsing it. An omitted value reads
/// as `Denied`.
#[derive(Debug, Clone, Copy, Default, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(from = "bool", into = "bool")]
pub enum AppleEventsAccess {
    #[default]
    Denied,
    Allowed,
}

impl AppleEventsAccess {
    pub const fn is_allowed(self) -> bool {
        matches!(self, Self::Allowed)
    }
}

impl From<bool> for AppleEventsAccess {
    fn from(flag: bool) -> Self {
        if flag { Self::Allowed } else { Self::Denied }
    }
}

impl From<AppleEventsAccess> for bool {
    fn from(value: AppleEventsAccess) -> Self {
        value.is_allowed()
    }
}

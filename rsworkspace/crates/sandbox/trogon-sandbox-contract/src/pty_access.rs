use serde::{Deserialize, Serialize};

/// Whether the payload may use pseudo-terminal devices on macOS.
///
/// It serializes as a JSON boolean, `true` for `Allowed`, so existing peers keep parsing it. An omitted value reads
/// as `Denied`.
#[derive(Debug, Clone, Copy, Default, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(from = "bool", into = "bool")]
pub enum PtyAccess {
    #[default]
    Denied,
    Allowed,
}

impl PtyAccess {
    pub const fn is_allowed(self) -> bool {
        matches!(self, Self::Allowed)
    }
}

impl From<bool> for PtyAccess {
    fn from(flag: bool) -> Self {
        if flag { Self::Allowed } else { Self::Denied }
    }
}

impl From<PtyAccess> for bool {
    fn from(value: PtyAccess) -> Self {
        value.is_allowed()
    }
}

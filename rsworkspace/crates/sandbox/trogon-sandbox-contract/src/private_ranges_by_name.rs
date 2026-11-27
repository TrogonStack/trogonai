use serde::{Deserialize, Serialize};

/// Whether a name may resolve into the private-use ranges (RFC 1918, CGNAT, unique local).
///
/// It serializes as a JSON boolean, `true` for `Enabled`, and an omitted value reads as `Disabled`.
#[derive(Debug, Clone, Copy, Default, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(from = "bool", into = "bool")]
pub enum PrivateRangesByName {
    #[default]
    Disabled,
    Enabled,
}

impl PrivateRangesByName {
    pub const fn is_enabled(&self) -> bool {
        matches!(self, Self::Enabled)
    }
}

impl From<bool> for PrivateRangesByName {
    fn from(enabled: bool) -> Self {
        if enabled { Self::Enabled } else { Self::Disabled }
    }
}

impl From<PrivateRangesByName> for bool {
    fn from(flag: PrivateRangesByName) -> Self {
        flag.is_enabled()
    }
}

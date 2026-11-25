use serde::{Deserialize, Serialize};

/// Whether the proxy applies its plaintext header set option.
///
/// It serializes as a JSON boolean, `true` for `Enabled`, and an omitted value reads as `Disabled`.
#[derive(Debug, Clone, Copy, Default, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(from = "bool", into = "bool")]
pub enum PlaintextHeaderSet {
    #[default]
    Disabled,
    Enabled,
}

impl PlaintextHeaderSet {
    pub const fn is_enabled(&self) -> bool {
        matches!(self, Self::Enabled)
    }
}

impl From<bool> for PlaintextHeaderSet {
    fn from(enabled: bool) -> Self {
        if enabled { Self::Enabled } else { Self::Disabled }
    }
}

impl From<PlaintextHeaderSet> for bool {
    fn from(flag: PlaintextHeaderSet) -> Self {
        flag.is_enabled()
    }
}

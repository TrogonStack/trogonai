use serde::{Deserialize, Serialize};

/// Whether a named destination is decided by its name alone, so an allowed IP literal or range never admits a name that resolves to it.
///
/// It serializes as a JSON boolean, `true` for `Enabled`, and an omitted value reads as `Disabled`.
#[derive(Debug, Clone, Copy, Default, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(from = "bool", into = "bool")]
pub enum NamesMatchedByName {
    #[default]
    Disabled,
    Enabled,
}

impl NamesMatchedByName {
    pub const fn is_enabled(&self) -> bool {
        matches!(self, Self::Enabled)
    }
}

impl From<bool> for NamesMatchedByName {
    fn from(enabled: bool) -> Self {
        if enabled { Self::Enabled } else { Self::Disabled }
    }
}

impl From<NamesMatchedByName> for bool {
    fn from(flag: NamesMatchedByName) -> Self {
        flag.is_enabled()
    }
}

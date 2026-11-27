use serde::{Deserialize, Serialize};

/// Whether a refused CONNECT carries its deny reason as the status phrase, and a 407 challenge goes out without a
/// mark.
///
/// It serializes as a JSON boolean, `true` for `Enabled`, and an omitted value reads as `Disabled`.
#[derive(Debug, Clone, Copy, Default, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(from = "bool", into = "bool")]
pub enum ReasonedRefusals {
    #[default]
    Disabled,
    Enabled,
}

impl ReasonedRefusals {
    pub const fn is_enabled(&self) -> bool {
        matches!(self, Self::Enabled)
    }
}

impl From<bool> for ReasonedRefusals {
    fn from(enabled: bool) -> Self {
        if enabled { Self::Enabled } else { Self::Disabled }
    }
}

impl From<ReasonedRefusals> for bool {
    fn from(flag: ReasonedRefusals) -> Self {
        flag.is_enabled()
    }
}

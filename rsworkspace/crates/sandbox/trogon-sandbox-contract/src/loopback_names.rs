use serde::{Deserialize, Serialize};

/// Whether `localhost` and every name under `.localhost` resolve to loopback and to nothing else.
///
/// It serializes as a JSON boolean, `true` for `Enabled`, and an omitted value reads as `Disabled`.
#[derive(Debug, Clone, Copy, Default, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(from = "bool", into = "bool")]
pub enum LoopbackNames {
    #[default]
    Disabled,
    Enabled,
}

impl LoopbackNames {
    pub const fn is_enabled(&self) -> bool {
        matches!(self, Self::Enabled)
    }
}

impl From<bool> for LoopbackNames {
    fn from(enabled: bool) -> Self {
        if enabled { Self::Enabled } else { Self::Disabled }
    }
}

impl From<LoopbackNames> for bool {
    fn from(flag: LoopbackNames) -> Self {
        flag.is_enabled()
    }
}

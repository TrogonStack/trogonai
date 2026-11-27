use serde::{Deserialize, Serialize};

/// Whether live flows are cancelled when a policy replacement no longer admits them or their name is no longer bound to their address.
///
/// It serializes as a JSON boolean, `true` for `Enabled`, and an omitted value reads as `Disabled`.
#[derive(Debug, Clone, Copy, Default, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(from = "bool", into = "bool")]
pub enum LiveFlowResweep {
    #[default]
    Disabled,
    Enabled,
}

impl LiveFlowResweep {
    pub const fn is_enabled(&self) -> bool {
        matches!(self, Self::Enabled)
    }
}

impl From<bool> for LiveFlowResweep {
    fn from(enabled: bool) -> Self {
        if enabled { Self::Enabled } else { Self::Disabled }
    }
}

impl From<LiveFlowResweep> for bool {
    fn from(flag: LiveFlowResweep) -> Self {
        flag.is_enabled()
    }
}

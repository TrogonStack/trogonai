use serde::{Deserialize, Serialize};

/// Whether a lease is a background pool maintenance lease that bypasses regular tenant quotas.
///
/// It serializes as a JSON boolean named `prewarm`, `true` for `Prewarm`. An omitted value reads as `Regular`.
#[derive(Debug, Clone, Copy, Default, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(from = "bool", into = "bool")]
pub enum LeasePrewarm {
    #[default]
    Regular,
    Prewarm,
}

impl LeasePrewarm {
    pub const fn is_prewarm(self) -> bool {
        matches!(self, Self::Prewarm)
    }
}

impl From<bool> for LeasePrewarm {
    fn from(flag: bool) -> Self {
        if flag { Self::Prewarm } else { Self::Regular }
    }
}

impl From<LeasePrewarm> for bool {
    fn from(value: LeasePrewarm) -> Self {
        value.is_prewarm()
    }
}

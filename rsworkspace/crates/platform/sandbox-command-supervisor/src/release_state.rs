use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "bool", into = "bool")]
pub enum ReleaseState {
    Held,
    Released,
}

impl From<bool> for ReleaseState {
    fn from(value: bool) -> Self {
        if value { Self::Released } else { Self::Held }
    }
}

impl From<ReleaseState> for bool {
    fn from(value: ReleaseState) -> Self {
        matches!(value, ReleaseState::Released)
    }
}

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "bool", into = "bool")]
pub enum Truncation {
    Complete,
    Truncated,
}

impl From<bool> for Truncation {
    fn from(value: bool) -> Self {
        if value { Self::Truncated } else { Self::Complete }
    }
}

impl From<Truncation> for bool {
    fn from(value: Truncation) -> Self {
        matches!(value, Truncation::Truncated)
    }
}

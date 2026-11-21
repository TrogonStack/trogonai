use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "bool", into = "bool")]
pub enum Cancellation {
    Active,
    Canceled,
}

impl From<bool> for Cancellation {
    fn from(value: bool) -> Self {
        if value { Self::Canceled } else { Self::Active }
    }
}

impl From<Cancellation> for bool {
    fn from(value: Cancellation) -> Self {
        matches!(value, Cancellation::Canceled)
    }
}

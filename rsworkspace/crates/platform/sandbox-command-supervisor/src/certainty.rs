use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "bool", into = "bool")]
pub enum Certainty {
    Certain,
    Uncertain,
}

impl From<bool> for Certainty {
    fn from(value: bool) -> Self {
        if value { Self::Uncertain } else { Self::Certain }
    }
}

impl From<Certainty> for bool {
    fn from(value: Certainty) -> Self {
        matches!(value, Certainty::Uncertain)
    }
}

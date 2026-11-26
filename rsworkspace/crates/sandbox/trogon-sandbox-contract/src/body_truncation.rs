use serde::{Deserialize, Serialize};

/// Whether a request body shown to a decider is the whole body.
///
/// It serializes as a JSON boolean, `true` for `Truncated`, and an omitted value reads as `Complete`.
#[derive(Debug, Clone, Copy, Default, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(from = "bool", into = "bool")]
pub enum BodyTruncation {
    #[default]
    Complete,
    Truncated,
}

impl BodyTruncation {
    pub const fn is_complete(&self) -> bool {
        matches!(self, Self::Complete)
    }
}

impl From<bool> for BodyTruncation {
    fn from(truncated: bool) -> Self {
        if truncated { Self::Truncated } else { Self::Complete }
    }
}

impl From<BodyTruncation> for bool {
    fn from(truncation: BodyTruncation) -> Self {
        matches!(truncation, BodyTruncation::Truncated)
    }
}

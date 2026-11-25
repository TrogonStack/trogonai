use serde::{Deserialize, Serialize};

/// Whether a lease needs one capability from a worker.
///
/// It serializes as a JSON boolean, `true` for `Required`, so existing peers keep parsing it.
#[derive(Debug, Clone, Copy, Default, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(from = "bool", into = "bool")]
pub enum CapabilityNeed {
    Required,
    #[default]
    NotRequired,
}

impl CapabilityNeed {
    pub const fn is_required(self) -> bool {
        matches!(self, Self::Required)
    }
}

impl From<bool> for CapabilityNeed {
    fn from(required: bool) -> Self {
        if required { Self::Required } else { Self::NotRequired }
    }
}

impl From<CapabilityNeed> for bool {
    fn from(need: CapabilityNeed) -> Self {
        need.is_required()
    }
}

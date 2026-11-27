use serde::{Deserialize, Serialize};

/// Whether canceling a create operation also cleans up a lease the operation already created.
///
/// It serializes as the JSON boolean `compensate_created`, `true` for `Compensate`. An omitted value reads as
/// `Keep`.
#[derive(Debug, Clone, Copy, Default, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(from = "bool", into = "bool")]
pub enum CreatedLeaseCompensation {
    #[default]
    Keep,
    Compensate,
}

impl CreatedLeaseCompensation {
    pub const fn is_compensate(self) -> bool {
        matches!(self, Self::Compensate)
    }
}

impl From<bool> for CreatedLeaseCompensation {
    fn from(flag: bool) -> Self {
        if flag { Self::Compensate } else { Self::Keep }
    }
}

impl From<CreatedLeaseCompensation> for bool {
    fn from(value: CreatedLeaseCompensation) -> Self {
        value.is_compensate()
    }
}

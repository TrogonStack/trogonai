use serde::{Deserialize, Serialize};

/// Whether a lease grant created the lease or returned one that already existed.
///
/// It serializes as a JSON boolean, `true` for `Created`. An omitted value reads as `Existing`, which is what a
/// named reconnect and an idempotent replay report, so one-time setup can be gated on `Created`.
#[derive(Debug, Clone, Copy, Default, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(from = "bool", into = "bool")]
pub enum LeaseCreation {
    #[default]
    Existing,
    Created,
}

impl LeaseCreation {
    pub const fn is_created(self) -> bool {
        matches!(self, Self::Created)
    }
}

impl From<bool> for LeaseCreation {
    fn from(flag: bool) -> Self {
        if flag { Self::Created } else { Self::Existing }
    }
}

impl From<LeaseCreation> for bool {
    fn from(value: LeaseCreation) -> Self {
        value.is_created()
    }
}

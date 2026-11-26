use serde::{Deserialize, Serialize};

/// Which way a freeze the broker has started moves the sandbox.
///
/// It serializes as a JSON boolean, `true` for `Resume`, so existing peers keep parsing it.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(from = "bool", into = "bool")]
pub enum FreezeAction {
    Pause,
    Resume,
}

impl FreezeAction {
    pub const fn resumes(self) -> bool {
        matches!(self, Self::Resume)
    }
}

impl From<bool> for FreezeAction {
    fn from(resume: bool) -> Self {
        if resume { Self::Resume } else { Self::Pause }
    }
}

impl From<FreezeAction> for bool {
    fn from(action: FreezeAction) -> Self {
        action.resumes()
    }
}

use serde::{Deserialize, Serialize};

/// Whether a caller can read a provider-native snapshot receipt.
///
/// It serializes as a JSON boolean, `true` for `Accepted`, so existing peers keep parsing it.
#[derive(Debug, Clone, Copy, Default, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(from = "bool", into = "bool")]
pub enum NativeSnapshotAcceptance {
    Accepted,
    #[default]
    Declined,
}

impl NativeSnapshotAcceptance {
    pub const fn is_accepted(self) -> bool {
        matches!(self, Self::Accepted)
    }
}

impl From<bool> for NativeSnapshotAcceptance {
    fn from(accepted: bool) -> Self {
        if accepted { Self::Accepted } else { Self::Declined }
    }
}

impl From<NativeSnapshotAcceptance> for bool {
    fn from(acceptance: NativeSnapshotAcceptance) -> Self {
        acceptance.is_accepted()
    }
}

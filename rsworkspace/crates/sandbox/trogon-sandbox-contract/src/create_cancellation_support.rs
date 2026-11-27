use serde::{Deserialize, Serialize};

/// Whether the broker can cancel a durable create operation.
///
/// It serializes as the JSON boolean `supports_create_cancellation`, `true` for `Supported`.
#[derive(Debug, Clone, Copy, Default, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(from = "bool", into = "bool")]
pub enum CreateCancellationSupport {
    #[default]
    Unsupported,
    Supported,
}

impl CreateCancellationSupport {
    pub const fn is_supported(self) -> bool {
        matches!(self, Self::Supported)
    }
}

impl From<bool> for CreateCancellationSupport {
    fn from(flag: bool) -> Self {
        if flag { Self::Supported } else { Self::Unsupported }
    }
}

impl From<CreateCancellationSupport> for bool {
    fn from(value: CreateCancellationSupport) -> Self {
        value.is_supported()
    }
}

use serde::{Deserialize, Serialize};

/// Whether the worker publishes incremental output chunks while a command runs.
///
/// It serializes as the JSON boolean `stream_output`. Polling and the final response behave the same either way;
/// streaming only adds a live side channel of [`CommandOutputChunk`](crate::CommandOutputChunk)s.
#[derive(Debug, Clone, Copy, Default, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(from = "bool", into = "bool")]
pub enum StreamedOutput {
    #[default]
    Disabled,
    Enabled,
}

impl StreamedOutput {
    pub const fn is_enabled(self) -> bool {
        matches!(self, Self::Enabled)
    }
}

impl From<bool> for StreamedOutput {
    fn from(value: bool) -> Self {
        if value { Self::Enabled } else { Self::Disabled }
    }
}

impl From<StreamedOutput> for bool {
    fn from(value: StreamedOutput) -> Self {
        value.is_enabled()
    }
}

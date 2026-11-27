use serde::Serialize;

/// Whether E2B pauses a sandbox when its lifetime ends instead of killing it.
///
/// It serializes as the JSON boolean `autoPause`, `true` for `Enabled`.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize)]
#[serde(into = "bool")]
pub enum AutoPause {
    Enabled,
    Disabled,
}

impl From<bool> for AutoPause {
    fn from(enabled: bool) -> Self {
        if enabled { Self::Enabled } else { Self::Disabled }
    }
}

impl From<AutoPause> for bool {
    fn from(value: AutoPause) -> Self {
        matches!(value, AutoPause::Enabled)
    }
}

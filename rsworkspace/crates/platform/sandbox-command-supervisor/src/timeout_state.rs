use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "bool", into = "bool")]
pub enum TimeoutState {
    WithinDeadline,
    TimedOut,
}

impl From<bool> for TimeoutState {
    fn from(value: bool) -> Self {
        if value { Self::TimedOut } else { Self::WithinDeadline }
    }
}

impl From<TimeoutState> for bool {
    fn from(value: TimeoutState) -> Self {
        matches!(value, TimeoutState::TimedOut)
    }
}

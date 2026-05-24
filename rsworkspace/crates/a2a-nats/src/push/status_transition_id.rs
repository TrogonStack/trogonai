use std::fmt;

use crate::push::terminal_push_task_state::TerminalPushTaskState;

/// Stable identifier for the terminal task status transition that triggered push dispatch.
#[derive(Clone, Eq, PartialEq)]
pub struct StatusTransitionId(String);

impl StatusTransitionId {
    pub fn new(raw: impl Into<String>) -> Self {
        Self(raw.into())
    }

    pub fn from_terminal(state: TerminalPushTaskState) -> Self {
        Self(state.idempotency_segment().to_owned())
    }

    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

impl fmt::Display for StatusTransitionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl fmt::Debug for StatusTransitionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("StatusTransitionId").field(&self.0).finish()
    }
}

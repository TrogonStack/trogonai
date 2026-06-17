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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_wraps_arbitrary_string_unchanged() {
        let id = StatusTransitionId::new("custom-token");
        assert_eq!(id.as_str(), "custom-token");
        assert_eq!(id.to_string(), "custom-token");
    }

    #[test]
    fn from_terminal_uses_idempotency_segment() {
        assert_eq!(
            StatusTransitionId::from_terminal(TerminalPushTaskState::Completed).as_str(),
            "completed"
        );
        assert_eq!(
            StatusTransitionId::from_terminal(TerminalPushTaskState::Failed).as_str(),
            "failed"
        );
    }

    #[test]
    fn debug_renders_tuple_with_inner_value() {
        let id = StatusTransitionId::new("rejected");
        assert!(format!("{id:?}").contains("rejected"));
    }
}

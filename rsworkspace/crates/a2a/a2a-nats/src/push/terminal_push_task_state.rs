use a2a::event::StreamResponse;
use a2a::types::TaskState;

/// Terminal task state emitted on the `message/stream` status update path (`PushDispatcher` eligibility).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TerminalPushTaskState {
    Completed,
    Failed,
    Canceled,
    Rejected,
}

impl TerminalPushTaskState {
    pub fn idempotency_segment(self) -> &'static str {
        match self {
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Canceled => "canceled",
            Self::Rejected => "rejected",
        }
    }

    pub fn from_stream_terminal_response(ev: &StreamResponse) -> Option<Self> {
        let StreamResponse::StatusUpdate(update) = ev else {
            return None;
        };
        match update.status.state {
            TaskState::Completed => Some(Self::Completed),
            TaskState::Failed => Some(Self::Failed),
            TaskState::Canceled => Some(Self::Canceled),
            TaskState::Rejected => Some(Self::Rejected),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests;

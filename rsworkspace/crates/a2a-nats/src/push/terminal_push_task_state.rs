use a2a_types::stream_response::Payload;
use a2a_types::{StreamResponse, TaskState};

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
        let Payload::StatusUpdate(update) = ev.payload.as_ref()? else {
            return None;
        };
        let state = update.status.as_ref().map(|s| s.state).unwrap_or(0);
        match TaskState::try_from(state).ok()? {
            TaskState::Completed => Some(Self::Completed),
            TaskState::Failed => Some(Self::Failed),
            TaskState::Canceled => Some(Self::Canceled),
            TaskState::Rejected => Some(Self::Rejected),
            _ => None,
        }
    }
}

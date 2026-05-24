use std::fmt;

use crate::push::push_notification_config_id::PushNotificationConfigId;
use crate::push::status_transition_id::StatusTransitionId;
use crate::push::terminal_push_task_state::TerminalPushTaskState;
use crate::task_id::A2aTaskId;

/// Stable `{task}:{push-config}:{terminal}` key for outbound terminal push deliveries.
#[derive(Clone, Eq, PartialEq)]
pub struct PushIdempotencyKey(String);

impl PushIdempotencyKey {
    pub fn derive_terminal(
        task_id: &A2aTaskId,
        cfg_id: &PushNotificationConfigId,
        terminal: TerminalPushTaskState,
    ) -> Self {
        let s = format!(
            "{}:{}:{}",
            task_id.as_str(),
            cfg_id.as_str(),
            terminal.idempotency_segment()
        );
        Self(s)
    }

    /// Stable DLQ dedupe key: `{task_id}:{status_transition_id}:{push_target_url}`.
    pub fn derive_dlq(task_id: &A2aTaskId, transition_id: &StatusTransitionId, push_target_url: &str) -> Self {
        Self(format!(
            "{}:{}:{}",
            task_id.as_str(),
            transition_id.as_str(),
            push_target_url
        ))
    }

    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }

    pub fn from_dedupe_wire(raw: impl Into<String>) -> Self {
        Self(raw.into())
    }
}

impl fmt::Display for PushIdempotencyKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl fmt::Debug for PushIdempotencyKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("PushIdempotencyKey").field(&self.0).finish()
    }
}

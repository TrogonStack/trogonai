use std::fmt;

use crate::push::push_notification_config_id::PushNotificationConfigId;
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

    pub fn as_str(&self) -> &str {
        self.0.as_str()
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

use std::fmt;

use crate::push::push_notification_config_id::PushNotificationConfigId;
use crate::push::status_transition_id::StatusTransitionId;
use crate::push::terminal_push_task_state::TerminalPushTaskState;
use crate::task_id::A2aTaskId;

/// Stable `{task}:{push-config}:{terminal}` key for outbound terminal push deliveries.
#[derive(Clone, Eq, PartialEq)]
pub struct PushIdempotencyKey(String);

impl PushIdempotencyKey {
    /// Stable terminal-delivery dedup key.
    ///
    /// Encoded as length-prefixed components so two `(task, cfg)` pairs that
    /// happen to contain the field separator on the wire (`A2aTaskId` accepts
    /// `:` per the underlying NATS token rules) can't collide with another
    /// pair whose colons fall in different places.
    pub fn derive_terminal(
        task_id: &A2aTaskId,
        cfg_id: &PushNotificationConfigId,
        terminal: TerminalPushTaskState,
    ) -> Self {
        Self(encode_components([
            TERMINAL_KIND,
            task_id.as_str(),
            cfg_id.as_str(),
            terminal.idempotency_segment(),
        ]))
    }

    /// Stable DLQ dedup key: same length-prefixed encoding as `derive_terminal`
    /// with a distinct kind discriminant so a terminal key and a DLQ key with
    /// equal-length components can't collide inside a shared dedupe store.
    ///
    /// `push_target_url` is accepted as `&str` here pending the validated
    /// `PushTargetUrl` value object that lands with the push-target PR; once
    /// that ships this parameter is retyped to `&PushTargetUrl`.
    pub fn derive_dlq(task_id: &A2aTaskId, transition_id: &StatusTransitionId, push_target_url: &str) -> Self {
        Self(encode_components([
            DLQ_KIND,
            task_id.as_str(),
            transition_id.as_str(),
            push_target_url,
        ]))
    }

    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }

    /// Reconstruct a key from a value we previously wrote to the dedupe store.
    ///
    /// This is the only constructor that skips the length-prefixed encoding —
    /// the wire-side store is the authoritative producer (every entry it
    /// returns originated from `derive_terminal` or `derive_dlq`), so
    /// re-validating here would just round-trip our own bytes. The intent is
    /// boundary-trusted reconstruction, not user-supplied input.
    pub fn from_dedupe_wire(raw: impl Into<String>) -> Self {
        Self(raw.into())
    }
}

const TERMINAL_KIND: &str = "terminal";
const DLQ_KIND: &str = "dlq";

/// `{len(comp[0])}:{comp[0]}|{len(comp[1])}:{comp[1]}|…` — injective because
/// every component starts with its byte length, so the parser can recover
/// each part regardless of which characters it contains.
fn encode_components<const N: usize>(components: [&str; N]) -> String {
    let mut out = String::new();
    for (i, c) in components.iter().enumerate() {
        if i > 0 {
            out.push('|');
        }
        out.push_str(&c.len().to_string());
        out.push(':');
        out.push_str(c);
    }
    out
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

#[cfg(test)]
mod tests;

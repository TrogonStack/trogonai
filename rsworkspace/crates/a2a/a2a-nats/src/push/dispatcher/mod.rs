//! Push dispatcher trait + shared retry / idempotency-key helpers.
//!
//! Transport-specific impls live in sibling modules — `http` ships now, NATS /
//! JetStream publish and the composite façade land in follow-up PRs so each
//! transport's wire contract is reviewed on its own.

pub mod composite;
pub mod http;
pub mod jetstream;
pub mod nats;

pub use composite::{CompositePushDispatcher, composite_push_dispatcher};
pub use http::HttpPushDispatcher;
pub use jetstream::JetStreamPublishPushDispatcher;
pub use nats::NatsPublishPushDispatcher;

use a2a::types::TaskPushNotificationConfig;

use crate::push::delivery_semantics::DeliverySemantics;
use crate::push::dispatch_error::{DispatchError, DispatchPrepError};
use crate::push::push_idempotency_key::PushIdempotencyKey;
use crate::push::push_notification_config_id::PushNotificationConfigId;
use crate::push::terminal_push_task_state::TerminalPushTaskState;
use crate::task_id::A2aTaskId;

#[async_trait::async_trait]
pub trait PushDispatcher: Send + Sync + 'static {
    async fn dispatch(
        &self,
        task_id: &A2aTaskId,
        config: &TaskPushNotificationConfig,
        delivery_semantics: DeliverySemantics,
        terminal_task_state: TerminalPushTaskState,
        payload: &[u8],
    ) -> Result<(), DispatchError>;
}

/// Derive the terminal-delivery idempotency key when delivery semantics demand
/// dedup. Returns `Ok(None)` for at-least-once semantics (no key required).
pub fn maybe_terminal_push_idempotency_key(
    config: &TaskPushNotificationConfig,
    task_id: &A2aTaskId,
    semantics: &DeliverySemantics,
    terminal_state: TerminalPushTaskState,
) -> Result<Option<PushIdempotencyKey>, DispatchPrepError> {
    if !semantics.idempotency_key_required() {
        return Ok(None);
    }
    let cid = PushNotificationConfigId::new(config.id.clone().unwrap_or_default())
        .map_err(DispatchPrepError::PushConfigId)?;
    Ok(Some(PushIdempotencyKey::derive_terminal(task_id, &cid, terminal_state)))
}

/// HTTP status codes the webhook dispatcher should retry on (RFC 9110 "transient
/// server error" set plus the Cloudflare-specific 522/524 timeouts).
pub fn webhook_http_retryable(status: u16) -> bool {
    matches!(status, 408 | 425 | 429 | 500 | 501 | 502 | 503 | 504 | 522 | 524)
}

#[cfg(test)]
mod tests;

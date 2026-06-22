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
pub mod tests {
    use std::sync::{Arc, Mutex};

    use super::*;
    use crate::push::idempotency_key_header::IdempotencyKeyHeader;
    use crate::push::push_idempotency_key::PushIdempotencyKey;
    use crate::push::terminal_push_task_state::TerminalPushTaskState;

    pub struct MockPushDispatcher {
        #[allow(clippy::type_complexity)]
        pub calls: Arc<
            Mutex<
                Vec<(
                    A2aTaskId,
                    DeliverySemantics,
                    TerminalPushTaskState,
                    TaskPushNotificationConfig,
                    Vec<u8>,
                )>,
            >,
        >,
        pub result: Arc<Mutex<Option<Result<(), String>>>>,
    }

    impl Default for MockPushDispatcher {
        fn default() -> Self {
            Self::new()
        }
    }

    impl MockPushDispatcher {
        pub fn new() -> Self {
            Self {
                calls: Arc::new(Mutex::new(vec![])),
                result: Arc::new(Mutex::new(None)),
            }
        }

        pub fn fail_with(error: impl Into<String>) -> Self {
            let d = Self::new();
            *d.result.lock().unwrap() = Some(Err(error.into()));
            d
        }

        #[allow(clippy::type_complexity)]
        pub fn recorded_calls(
            &self,
        ) -> Vec<(
            A2aTaskId,
            DeliverySemantics,
            TerminalPushTaskState,
            TaskPushNotificationConfig,
            Vec<u8>,
        )> {
            self.calls.lock().unwrap().clone()
        }
    }

    #[async_trait::async_trait]
    impl PushDispatcher for MockPushDispatcher {
        async fn dispatch(
            &self,
            task_id: &A2aTaskId,
            config: &TaskPushNotificationConfig,
            delivery_semantics: DeliverySemantics,
            terminal_task_state: TerminalPushTaskState,
            payload: &[u8],
        ) -> Result<(), DispatchError> {
            self.calls.lock().unwrap().push((
                task_id.clone(),
                delivery_semantics,
                terminal_task_state,
                config.clone(),
                payload.to_vec(),
            ));
            match self.result.lock().unwrap().take() {
                Some(Err(msg)) => Err(DispatchError::UnexpectedStatus {
                    status: 500,
                    url: crate::push::target::WebhookUrl::new(&msg)
                        .unwrap_or_else(|_| crate::push::target::WebhookUrl::new("https://mock/").unwrap()),
                }),
                _ => Ok(()),
            }
        }
    }


    fn config_with_id(id: Option<&str>) -> TaskPushNotificationConfig {
        TaskPushNotificationConfig {
            url: "https://example.com/hook".into(),
            id: id.map(str::to_owned),
            task_id: String::new(),
            token: None,
            authentication: None,
            tenant: None,
        }
    }

    fn task() -> A2aTaskId {
        A2aTaskId::new("task-1").unwrap()
    }

    #[test]
    fn maybe_terminal_returns_none_for_at_least_once_semantics() {
        let config = config_with_id(Some("cfg-1"));
        let result = maybe_terminal_push_idempotency_key(
            &config,
            &task(),
            &DeliverySemantics::AtLeastOnce,
            TerminalPushTaskState::Completed,
        )
        .unwrap();
        assert!(result.is_none(), "AtLeastOnce must not derive an idempotency key");
    }

    #[test]
    fn maybe_terminal_returns_key_for_exactly_once_semantics() {
        let config = config_with_id(Some("cfg-1"));
        let key = maybe_terminal_push_idempotency_key(
            &config,
            &task(),
            &DeliverySemantics::ExactlyOnce {
                idempotency_key_header: None,
            },
            TerminalPushTaskState::Completed,
        )
        .unwrap()
        .expect("exactly-once must derive a key");
        let expected = PushIdempotencyKey::derive_terminal(
            &task(),
            &PushNotificationConfigId::new("cfg-1").unwrap(),
            TerminalPushTaskState::Completed,
        );
        assert_eq!(key.as_str(), expected.as_str());
    }

    #[test]
    fn maybe_terminal_returns_key_when_config_id_is_missing_via_default() {
        // A missing/None config id defaults to the empty string at the wire
        // boundary, which `PushNotificationConfigId::new` rejects with Empty;
        // the helper must propagate that as DispatchPrepError::PushConfigId.
        let config = config_with_id(None);
        let err = maybe_terminal_push_idempotency_key(
            &config,
            &task(),
            &DeliverySemantics::ExactlyOnce {
                idempotency_key_header: None,
            },
            TerminalPushTaskState::Completed,
        )
        .unwrap_err();
        assert!(matches!(err, DispatchPrepError::PushConfigId(_)));
    }

    #[test]
    fn maybe_terminal_propagates_custom_idempotency_header_under_exactly_once() {
        // The carrier header doesn't affect key derivation, but the helper
        // still emits a key whenever exactly-once is requested with any header.
        let config = config_with_id(Some("cfg-2"));
        let key = maybe_terminal_push_idempotency_key(
            &config,
            &task(),
            &DeliverySemantics::ExactlyOnce {
                idempotency_key_header: Some(IdempotencyKeyHeader::try_from("X-Push-Key").unwrap()),
            },
            TerminalPushTaskState::Failed,
        )
        .unwrap();
        assert!(key.is_some());
    }

    #[test]
    fn webhook_http_retryable_returns_true_for_documented_retry_set() {
        for status in [408u16, 425, 429, 500, 501, 502, 503, 504, 522, 524] {
            assert!(webhook_http_retryable(status), "{status} must be in the retry set");
        }
    }

    #[test]
    fn webhook_http_retryable_returns_false_for_terminal_statuses() {
        for status in [200u16, 201, 301, 400, 401, 403, 404, 410, 418, 426, 451] {
            assert!(!webhook_http_retryable(status), "{status} must not be retried");
        }
    }
}

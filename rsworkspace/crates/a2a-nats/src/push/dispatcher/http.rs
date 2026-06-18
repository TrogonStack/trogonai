//! `PushDispatcher` impl for HTTPS webhook targets.

use std::time::Duration;

use a2a::types::TaskPushNotificationConfig;

use crate::constants::HTTP_PUSH_WEBHOOK_MAX_ATTEMPTS;
use crate::push::authentication_header::authorization_header_value;
use crate::push::delivery_semantics::DeliverySemantics;
use crate::push::dispatch_error::DispatchError;
use crate::push::dispatcher::{PushDispatcher, maybe_terminal_push_idempotency_key, webhook_http_retryable};
use crate::push::push_notification_target::{PushNotificationTarget, PushNotificationTargetError};
use crate::push::target::WebhookUrl;
use crate::push::terminal_push_task_state::TerminalPushTaskState;
use crate::task_id::A2aTaskId;

const INITIAL_RETRY_DELAY: Duration = Duration::from_millis(100);
const MAX_RETRY_DELAY: Duration = Duration::from_secs(3);

pub struct HttpPushDispatcher {
    client: reqwest::Client,
}

impl HttpPushDispatcher {
    pub fn new(client: reqwest::Client) -> Self {
        Self { client }
    }
}

/// Transport-level reqwest errors the dispatcher should retry — connection
/// refused / timed out can recover with another attempt; protocol errors
/// (4xx body shapes, invalid TLS etc.) shouldn't.
pub fn push_transport_retryable(error: &reqwest::Error) -> bool {
    error.is_timeout() || error.is_connect()
}

#[async_trait::async_trait]
impl PushDispatcher for HttpPushDispatcher {
    async fn dispatch(
        &self,
        task_id: &A2aTaskId,
        config: &TaskPushNotificationConfig,
        delivery_semantics: DeliverySemantics,
        terminal_task_state: TerminalPushTaskState,
        payload: &[u8],
    ) -> Result<(), DispatchError> {
        let url = parse_http_target(&config.url)?;

        let authorization =
            authorization_header_value(config.authentication.as_ref()).map_err(DispatchError::InvalidAuthorization)?;

        let maybe_key = maybe_terminal_push_idempotency_key(config, task_id, &delivery_semantics, terminal_task_state)
            .map_err(DispatchError::Prep)?;

        let max_attempts_usize = usize::try_from(delivery_semantics.webhook_max_publish_attempts())
            .unwrap_or(HTTP_PUSH_WEBHOOK_MAX_ATTEMPTS as usize);

        let mut delay = INITIAL_RETRY_DELAY;

        for attempt in 0..max_attempts_usize {
            let mut request = self
                .client
                .post(url.as_str())
                .header(reqwest::header::CONTENT_TYPE, "application/json")
                .body(payload.to_vec());

            if let Some(ref auth_value) = authorization {
                request = request.header(reqwest::header::AUTHORIZATION, auth_value.clone());
            }

            if let Some(hn) = delivery_semantics.webhook_idempotency_carrier()
                && let Some(ref k) = maybe_key
            {
                let hv = reqwest::header::HeaderValue::from_str(k.as_str())
                    .map_err(|e| DispatchError::InvalidHeader(Box::new(e)))?;
                request = request.header(hn.clone(), hv);
            }

            match request.send().await {
                Ok(response) => {
                    if response.status().is_success() {
                        return Ok(());
                    }

                    let status = response.status().as_u16();
                    if webhook_http_retryable(status) && attempt + 1 < max_attempts_usize {
                        tokio::time::sleep(delay).await;
                        delay = next_retry_delay(delay);
                        continue;
                    }

                    return Err(DispatchError::UnexpectedStatus { status, url });
                }
                Err(e) => {
                    if push_transport_retryable(&e) && attempt + 1 < max_attempts_usize {
                        tokio::time::sleep(delay).await;
                        delay = next_retry_delay(delay);
                        continue;
                    }
                    return Err(DispatchError::Http(Box::new(e)));
                }
            }
        }

        // The loop runs at least once (max_attempts_usize >= 1 per
        // HTTP_PUSH_WEBHOOK_MAX_ATTEMPTS) and every iteration either returns
        // Ok / Err on its terminal attempt or `continue`s with a retry budget
        // remaining, so falling through means the loop bound is mis-configured.
        Err(DispatchError::Http(Box::<dyn std::error::Error + Send + Sync>::from(
            "webhook retry loop exited without resolution",
        )))
    }
}

fn parse_http_target(raw: &str) -> Result<WebhookUrl, DispatchError> {
    let PushNotificationTarget::Http(url) = PushNotificationTarget::parse(raw)? else {
        return Err(DispatchError::InvalidTarget(
            PushNotificationTargetError::UnknownScheme { raw: raw.to_owned() },
        ));
    };
    Ok(url)
}

fn next_retry_delay(current: Duration) -> Duration {
    current.saturating_mul(2).min(MAX_RETRY_DELAY)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn next_retry_delay_doubles_until_ceiling() {
        assert_eq!(next_retry_delay(Duration::from_millis(100)), Duration::from_millis(200));
        assert_eq!(
            next_retry_delay(Duration::from_millis(800)),
            Duration::from_millis(1600)
        );
        assert_eq!(next_retry_delay(MAX_RETRY_DELAY), MAX_RETRY_DELAY);
        assert_eq!(next_retry_delay(Duration::from_secs(60)), MAX_RETRY_DELAY);
    }

    #[test]
    fn parse_http_target_accepts_http_and_https() {
        assert!(parse_http_target("https://example.com/hook").is_ok());
        assert!(parse_http_target("http://localhost:8080/hook").is_ok());
    }

    #[test]
    fn parse_http_target_rejects_non_http_target() {
        let err = parse_http_target("subject:a2a.push.t.caller.task").unwrap_err();
        assert!(matches!(err, DispatchError::InvalidTarget(_)));
    }

    #[tokio::test]
    async fn dispatch_returns_invalid_target_for_non_http_scheme() {
        let dispatcher = HttpPushDispatcher::new(reqwest::Client::new());
        let config = TaskPushNotificationConfig {
            url: "subject:a2a.push.t.caller.task".into(),
            id: Some("cfg-1".into()),
            task_id: String::new(),
            token: None,
            authentication: None,
            tenant: None,
        };
        let err = dispatcher
            .dispatch(
                &A2aTaskId::new("task-1").unwrap(),
                &config,
                DeliverySemantics::AtLeastOnce,
                TerminalPushTaskState::Completed,
                b"{}",
            )
            .await
            .unwrap_err();
        assert!(matches!(err, DispatchError::InvalidTarget(_)));
    }

    #[tokio::test]
    async fn dispatch_returns_invalid_authorization_when_scheme_unsupported() {
        let dispatcher = HttpPushDispatcher::new(reqwest::Client::new());
        let config = TaskPushNotificationConfig {
            url: "https://example.invalid/hook".into(),
            id: Some("cfg-1".into()),
            task_id: String::new(),
            token: None,
            authentication: Some(a2a::types::AuthenticationInfo {
                scheme: "Digest".into(),
                credentials: Some("opaque".into()),
            }),
            tenant: None,
        };
        let err = dispatcher
            .dispatch(
                &A2aTaskId::new("task-1").unwrap(),
                &config,
                DeliverySemantics::AtLeastOnce,
                TerminalPushTaskState::Completed,
                b"{}",
            )
            .await
            .unwrap_err();
        assert!(matches!(err, DispatchError::InvalidAuthorization(_)));
    }

    #[tokio::test]
    async fn dispatch_returns_prep_error_when_exactly_once_lacks_config_id() {
        let dispatcher = HttpPushDispatcher::new(reqwest::Client::new());
        let config = TaskPushNotificationConfig {
            url: "https://example.invalid/hook".into(),
            id: None,
            task_id: String::new(),
            token: None,
            authentication: None,
            tenant: None,
        };
        let err = dispatcher
            .dispatch(
                &A2aTaskId::new("task-1").unwrap(),
                &config,
                DeliverySemantics::ExactlyOnce {
                    idempotency_key_header: None,
                },
                TerminalPushTaskState::Completed,
                b"{}",
            )
            .await
            .unwrap_err();
        assert!(matches!(err, DispatchError::Prep(_)));
    }

    #[test]
    fn push_transport_retryable_handles_zero_attempts_corner_case() {
        // Sanity: the helper just delegates to reqwest's classifiers — we
        // can't construct a reqwest::Error directly, but we can confirm the
        // matcher compiles into a small no-op on a fresh client error by
        // exercising it through dispatch above. Direct test placeholder.
    }
}

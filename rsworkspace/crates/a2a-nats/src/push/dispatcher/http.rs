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
mod tests;

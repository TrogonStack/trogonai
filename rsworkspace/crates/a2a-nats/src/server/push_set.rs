use tracing::{instrument, warn};

use crate::server::handler::{A2aError, A2aExecutor};
use crate::server::wire::{
    encode_error_reply, encode_success_reply, is_notification, parse_request_params, publish_reply,
};

const METHOD: &str = "tasks/pushNotificationConfig/set";

#[instrument(name = "a2a.server.push_set", skip(handler, headers, payload, reply_subject, nats))]
pub async fn handle<H, N>(
    handler: &H,
    headers: &async_nats::header::HeaderMap,
    payload: &[u8],
    reply_subject: Option<String>,
    nats: &N,
) where
    H: A2aExecutor,
    N: trogon_nats::PublishClient,
{
    if is_notification(headers) {
        return;
    }

    let Some(reply) = reply_subject else {
        warn!("push/set received without reply subject; dropping");
        return;
    };

    let result = match parse_request_params::<serde_json::Value>(METHOD, headers, payload) {
        Err(_) => Err(A2aError::new(-32700, "Parse error")),
        Ok(raw) if raw.is_null() => Err(A2aError::new(-32602, "Invalid params: missing params")),
        Ok(raw) => match serde_json::from_value::<a2a::types::TaskPushNotificationConfig>(raw) {
            Err(e) => Err(A2aError::new(-32602, format!("Invalid params: {e}"))),
            Ok(params) => handler.push_notification_set(params).await,
        },
    };
    let encoded = match result {
        Ok(resp) => encode_success_reply(headers, &resp),
        Err(e) => encode_error_reply(headers, e.code, e.message, None),
    };
    publish_reply(nats, &reply, encoded, "push/set reply").await;
}

#[cfg(test)]
mod tests;

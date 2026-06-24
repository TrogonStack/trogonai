use tracing::{instrument, warn};

use crate::server::handler::{A2aError, A2aExecutor};
use crate::server::wire::{
    encode_error_reply, encode_success_reply, is_notification, parse_request_params, publish_reply, request_id,
};

const METHOD: &str = "message/send";

#[instrument(name = "a2a.server.message_send", skip(handler, headers, payload, reply_subject, nats))]
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
    let Some(reply) = reply_subject else {
        warn!("message/send received without reply subject; dropping");
        return;
    };

    if request_id(headers).is_none() && is_notification(headers) {
        return;
    }

    let result = match parse_request_params::<serde_json::Value>(METHOD, headers, payload) {
        Err(_) => Err(parse_error()),
        Ok(raw) if raw.is_null() => Err(A2aError::new(-32602, "Invalid params: missing params")),
        Ok(raw) => match serde_json::from_value::<a2a::types::SendMessageRequest>(raw) {
            Err(e) => Err(A2aError::new(-32602, format!("Invalid params: {e}"))),
            Ok(params) => handler.message_send(params).await,
        },
    };
    let encoded = match result {
        Ok(resp) => encode_success_reply(headers, &resp),
        Err(e) => encode_error_reply(headers, e.code, e.message, None),
    };
    publish_reply(nats, &reply, encoded, "message/send reply").await;
}

fn parse_error() -> A2aError {
    A2aError::new(-32700, "Parse error")
}

#[cfg(test)]
mod tests;

use tracing::{instrument, warn};

use crate::jsonrpc::extract_request_id;
use crate::server::handler::{A2aError, A2aExecutor};
use crate::server::wire::{JsonRpcErrorResponse, JsonRpcResponse, is_notification, parse_request};

/// Handles `tasks/pushNotificationConfig/set`.
///
/// Delivery-semantics extension wiring (registry tracking, `deliverySemantics`
/// extension field) lands in its own follow-up so this slice stays on the
/// minimal unary template the other per-op handlers use.
#[instrument(name = "a2a.server.push_set", skip(handler, payload, reply_subject, nats))]
pub async fn handle<H, N>(handler: &H, payload: &[u8], reply_subject: Option<String>, nats: &N)
where
    H: A2aExecutor,
    N: trogon_nats::PublishClient,
{
    let Some(reply) = reply_subject else {
        warn!("tasks/pushNotificationConfig/set received without reply subject; dropping");
        return;
    };

    let id = extract_request_id(payload);
    if id.is_none() && is_notification(payload) {
        return;
    }

    // Parse the envelope first as a generic Value so we can distinguish
    // malformed JSON (-32700) from a well-formed envelope with an invalid
    // params shape (-32602).
    let result = match parse_request::<serde_json::Value>(payload) {
        Err(_) => Err(A2aError::new(-32700, "Parse error")),
        Ok(envelope) => match envelope.params {
            None => Err(A2aError::new(-32602, "Invalid params: missing params")),
            Some(raw) => match serde_json::from_value::<a2a::types::TaskPushNotificationConfig>(raw) {
                Err(e) => Err(A2aError::new(-32602, format!("Invalid params: {e}"))),
                Ok(params) => handler.push_notification_set(params).await,
            },
        },
    };
    let bytes = match result {
        Ok(resp) => JsonRpcResponse::new(id, resp).to_bytes(),
        Err(e) => JsonRpcErrorResponse::new(id, e.code, e.message).to_bytes(),
    };
    match bytes {
        Ok(b) => {
            let headers = async_nats::HeaderMap::new();
            if let Err(e) = nats
                .publish_with_headers(async_nats::Subject::from(reply.as_str()), headers, b)
                .await
            {
                warn!(error = %e, "failed to publish push_set reply");
            }
        }
        Err(e) => warn!(error = %e, "failed to serialize push_set response"),
    }
}

#[cfg(test)]
mod tests;

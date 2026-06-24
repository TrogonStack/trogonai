use crate::constants::{CONTENT_TYPE_JSON, CONTENT_TYPE_PLAIN};
use crate::nats::{FlushClient, PublishClient, headers_with_trace_context};
use crate::wire::{encode_agent_error, encode_success, merge_jsonrpc_headers};
use agent_client_protocol::{Error, ErrorCode};
use async_nats::header::HeaderMap;
use jsonrpc_nats::{Encoded, ResponseId};
use tracing::warn;

fn reply_headers(wire_headers: &HeaderMap) -> HeaderMap {
    let mut headers = merge_jsonrpc_headers(headers_with_trace_context(), wire_headers.clone());
    headers.insert("Content-Type", CONTENT_TYPE_JSON);
    headers
}

pub async fn publish_wire_reply<N: PublishClient + FlushClient>(
    nats: &N,
    reply_to: &str,
    encoded: Encoded,
    context: &str,
) {
    let headers = reply_headers(&encoded.headers);
    if let Err(e) = nats.publish_with_headers(reply_to.to_string(), headers, encoded.body).await {
        warn!(error = %e, "Failed to publish {}", context);
    }
    if let Err(e) = nats.flush().await {
        warn!(error = %e, "Failed to flush {}", context);
    }
}

pub async fn publish_success_reply<N, Res>(
    nats: &N,
    reply_to: &str,
    response_id: ResponseId,
    result: &Res,
    context: &str,
) where
    N: PublishClient + FlushClient,
    Res: serde::Serialize,
{
    match encode_success(response_id.clone(), result) {
        Ok(encoded) => publish_wire_reply(nats, reply_to, encoded, context).await,
        Err(e) => {
            warn!(error = %e, "Failed to encode success reply for {}", context);
            publish_internal_error_reply(nats, reply_to, response_id, context).await;
        }
    }
}

pub async fn publish_agent_error_reply<N: PublishClient + FlushClient>(
    nats: &N,
    reply_to: &str,
    response_id: ResponseId,
    error: &Error,
    context: &str,
) {
    match encode_agent_error(response_id, error) {
        Ok(encoded) => publish_wire_reply(nats, reply_to, encoded, context).await,
        Err(e) => {
            warn!(error = %e, "Failed to encode error reply for {}", context);
            publish_fallback_error_reply(nats, reply_to, context).await;
        }
    }
}

pub async fn publish_error_reply<N: PublishClient + FlushClient>(
    nats: &N,
    reply_to: &str,
    response_id: ResponseId,
    code: ErrorCode,
    message: &str,
    context: &str,
) {
    let error = Error::new(i32::from(code), message);
    publish_agent_error_reply(nats, reply_to, response_id, &error, context).await;
}

async fn publish_internal_error_reply<N: PublishClient + FlushClient>(
    nats: &N,
    reply_to: &str,
    response_id: ResponseId,
    context: &str,
) {
    publish_error_reply(
        nats,
        reply_to,
        response_id,
        ErrorCode::InternalError,
        "Internal error",
        context,
    )
    .await;
}

async fn publish_fallback_error_reply<N: PublishClient + FlushClient>(nats: &N, reply_to: &str, context: &str) {
    let encoded = match encode_agent_error(
        ResponseId::Null,
        &Error::new(ErrorCode::InternalError.into(), "Internal error"),
    ) {
        Ok(encoded) => encoded,
        Err(e) => {
            warn!(error = %e, "Fallback wire encoding failed for {}", context);
            let mut headers = headers_with_trace_context();
            headers.insert("Content-Type", CONTENT_TYPE_PLAIN);
            if let Err(e) = nats
                .publish_with_headers(reply_to.to_string(), headers, "Internal error".into())
                .await
            {
                warn!(error = %e, "Failed to publish fallback {}", context);
            }
            if let Err(e) = nats.flush().await {
                warn!(error = %e, "Failed to flush fallback {}", context);
            }
            return;
        }
    };
    publish_wire_reply(nats, reply_to, encoded, context).await;
}

#[cfg(test)]
pub fn encode_success_for_test<Res: serde::Serialize>(
    response_id: ResponseId,
    result: &Res,
) -> Result<Encoded, crate::wire::WireError> {
    encode_success(response_id, result)
}

#[cfg(test)]
pub fn encode_agent_error_for_test(
    response_id: ResponseId,
    error: &Error,
) -> Result<Encoded, crate::wire::WireError> {
    encode_agent_error(response_id, error)
}

#[cfg(test)]
mod tests;

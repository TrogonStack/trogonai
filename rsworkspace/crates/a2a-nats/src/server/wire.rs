//! Server-side JSON-RPC content-mode wire helpers for A2A over NATS.

use async_nats::header::HeaderMap;
use jsonrpc_nats::Encoded;
use trogon_nats::PublishClient;

use crate::jsonrpc::JsonRpcId;
use crate::wire::{WireError, encode_error, encode_success, response_id_from_request_headers};

pub use crate::wire::{WireError as ServerWireError, decode_request_params as parse_request_params, is_notification};

/// Publish a content-mode JSON-RPC reply to the caller inbox.
pub async fn publish_reply<N: PublishClient>(
    nats: &N,
    reply: &str,
    encoded: Result<Encoded, WireError>,
    label: &'static str,
) {
    let encoded = match encoded {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(error = %e, label, "failed to encode reply");
            return;
        }
    };
    if let Err(e) = nats
        .publish_with_headers(
            async_nats::Subject::from(reply),
            encoded.headers,
            encoded.body,
        )
        .await
    {
        tracing::warn!(error = %e, label, "failed to publish reply");
    }
}

pub fn encode_success_reply<Res: serde::Serialize>(
    request_headers: &HeaderMap,
    result: &Res,
) -> Result<Encoded, WireError> {
    encode_success(response_id_from_request_headers(request_headers), result)
}

pub fn encode_error_reply(
    request_headers: &HeaderMap,
    code: i32,
    message: impl Into<String>,
    data: Option<serde_json::Value>,
) -> Result<Encoded, WireError> {
    encode_error(
        response_id_from_request_headers(request_headers),
        code,
        message,
        data,
    )
}

pub fn request_id(headers: &HeaderMap) -> Option<JsonRpcId> {
    crate::jsonrpc::extract_request_id(headers)
}

#[cfg(test)]
mod tests;

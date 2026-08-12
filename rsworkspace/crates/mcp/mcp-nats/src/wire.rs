//! MCP canonical JSON-RPC over NATS codec bridge.

use async_nats::header::HeaderMap;
use jsonrpc_nats::{CodecError, Direction, Encoded, decode_value, encode_value};
use rmcp::model::{GetExtensions, JsonRpcMessage};
use rmcp::service::{RxJsonRpcMessage, ServiceRole, TxJsonRpcMessage};

use crate::{McpTransportHeaders, transport::NatsTransportError};

pub use jsonrpc_nats::merge_headers;

pub fn encode_tx<R>(item: &TxJsonRpcMessage<R>) -> Result<Encoded, NatsTransportError>
where
    R: ServiceRole,
    R::Not: GetExtensions,
{
    let value = serde_json::to_value(item).map_err(NatsTransportError::Serialize)?;
    let mut encoded = encode_value(&value).map_err(map_codec_error)?;
    let transport_headers = match item {
        JsonRpcMessage::Request(request) => request.request.extensions().get::<McpTransportHeaders>(),
        JsonRpcMessage::Notification(notification) => {
            notification.notification.extensions().get::<McpTransportHeaders>()
        }
        JsonRpcMessage::Response(_) | JsonRpcMessage::Error(_) => None,
    };
    if let Some(headers) = transport_headers {
        headers.extend_nats(&mut encoded.headers);
    }
    Ok(encoded)
}

pub fn decode_rx<R: ServiceRole>(
    direction: Direction,
    method: Option<&str>,
    headers: &HeaderMap,
    body: &[u8],
) -> Result<RxJsonRpcMessage<R>, NatsTransportError> {
    let value = decode_value(direction, method, headers, body).map_err(map_codec_error)?;
    let mut item: RxJsonRpcMessage<R> = serde_json::from_value(value).map_err(NatsTransportError::Deserialize)?;
    let transport_headers = McpTransportHeaders::from_nats(headers);
    if !transport_headers.is_empty() {
        match &mut item {
            JsonRpcMessage::Request(request) => {
                request.request.extensions_mut().insert(transport_headers);
            }
            JsonRpcMessage::Notification(notification) => {
                notification.notification.extensions_mut().insert(transport_headers);
            }
            JsonRpcMessage::Response(_) | JsonRpcMessage::Error(_) => {}
        }
    }
    Ok(item)
}

fn map_codec_error(error: CodecError) -> NatsTransportError {
    NatsTransportError::Codec(error)
}

#[cfg(test)]
mod tests;

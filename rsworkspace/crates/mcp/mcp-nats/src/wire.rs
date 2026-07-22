//! MCP JSON-RPC ↔ jsonrpc-nats codec bridge.

use async_nats::header::HeaderMap;
use jsonrpc_nats::{CodecError, Direction, Encoded, decode, encode, from_json_value, to_json_value};
use rmcp::service::{RxJsonRpcMessage, ServiceRole, TxJsonRpcMessage};

use crate::transport::NatsTransportError;

pub use jsonrpc_nats::merge_jsonrpc_headers as merge_headers;

pub fn encode_tx<R: ServiceRole>(item: &TxJsonRpcMessage<R>) -> Result<Encoded, NatsTransportError> {
    let value = serde_json::to_value(item).map_err(NatsTransportError::Serialize)?;
    let message = from_json_value(&value).map_err(map_codec_error)?;
    encode(&message).map_err(map_codec_error)
}

pub fn decode_rx<R: ServiceRole>(
    direction: Direction,
    method: Option<&str>,
    headers: &HeaderMap,
    body: &[u8],
) -> Result<RxJsonRpcMessage<R>, NatsTransportError> {
    let message = decode(direction, method, headers, body).map_err(map_codec_error)?;
    let value = to_json_value(&message);
    serde_json::from_value(value).map_err(NatsTransportError::Deserialize)
}

fn map_codec_error(error: CodecError) -> NatsTransportError {
    NatsTransportError::Codec(error)
}

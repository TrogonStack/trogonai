//! Client-side JSON-RPC content-mode wire helpers.

pub use crate::wire::{WireError, decode_response, encode_request, merge_jsonrpc_headers};

use async_nats::header::HeaderMap;
use jsonrpc_nats::{Encoded, RequestId};
use serde::{Serialize, de::DeserializeOwned};

use crate::jsonrpc::JsonRpcId;

pub fn encode_client_request<Req: Serialize>(method: &str, id: JsonRpcId, params: &Req) -> Result<Encoded, WireError> {
    let request_id = match id {
        JsonRpcId::Number(n) => RequestId::Number(n),
        JsonRpcId::String(s) => RequestId::String(s),
        JsonRpcId::Null => {
            return Err(WireError::Codec(jsonrpc_nats::CodecError::RequestWithoutId));
        }
    };
    encode_request(method, request_id, params)
}

pub fn decode_client_response<Res: DeserializeOwned>(
    headers: &HeaderMap,
    body: &[u8],
) -> Result<Result<Res, (i32, String)>, WireError> {
    decode_response(headers, body)
}

#[cfg(test)]
mod tests;

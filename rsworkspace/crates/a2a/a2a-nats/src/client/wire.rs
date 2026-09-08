//! Client-side canonical JSON-RPC wire helpers (ADR#0056).

pub use crate::wire::{WireError, decode_response, encode_request, merge_jsonrpc_headers};

use async_nats::header::HeaderMap;
use jsonrpc_nats::{Encoded, RequestId};
use serde::{Serialize, de::DeserializeOwned};

use crate::client::error::ClientError;

pub fn encode_client_request<Req: Serialize>(method: &str, id: RequestId, params: &Req) -> Result<Encoded, WireError> {
    encode_request(method, id, params)
}

pub fn decode_client_response<Res: DeserializeOwned>(
    headers: &HeaderMap,
    body: &[u8],
) -> Result<Result<Res, (i32, String)>, WireError> {
    decode_response(headers, body)
}

/// The single wire-to-client error mapping. Every variant keeps its own
/// identity: flattening `Codec` and `UnexpectedMessage` into a synthesized
/// `Deserialize` would report a peer that broke the JSON-RPC framing as a
/// caller whose result type did not match, and callers cannot tell those apart
/// from a string.
pub fn map_wire_error(error: WireError) -> ClientError {
    match error {
        WireError::Serialize(e) => ClientError::Serialize(e),
        WireError::Deserialize(e) => ClientError::Deserialize(e),
        WireError::Codec(e) => ClientError::Codec(e),
        WireError::UnexpectedMessage => ClientError::UnexpectedMessage,
    }
}

#[cfg(test)]
mod tests;

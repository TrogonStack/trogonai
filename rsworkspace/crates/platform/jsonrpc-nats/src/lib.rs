//! JSON-RPC 2.0 over NATS codecs.
//!
//! The legacy [`encode`] and [`decode`] APIs retain ADR#0011 content mode for ACP
//! and A2A compatibility. The canonical APIs carry a complete JSON-RPC object
//! in the body for protocols whose transport contract requires the body to
//! remain authoritative.
#![cfg_attr(test, allow(clippy::expect_used, clippy::panic, clippy::unwrap_used))]

pub mod codec;
pub mod constants;
pub mod direction;
pub mod error;
pub mod id;
pub mod message;
pub mod transport;

pub use codec::{
    Encoded, decode, decode_canonical, decode_canonical_value, encode, encode_canonical, encode_canonical_value,
    from_json_value, to_json_value,
};
pub use constants::{HEADER_ERROR_CODE, HEADER_ID, JSONRPC_VERSION};
pub use direction::Direction;
pub use error::CodecError;
pub use id::{RequestId, ResponseId, decode_response_id_literal, encode_id_literal, encode_response_id_literal};
pub use message::Message;
pub use transport::{
    TransportError, jsonrpc_publish, jsonrpc_publish_with_timeout, jsonrpc_request_raw, jsonrpc_request_with_timeout,
    merge_headers, merge_jsonrpc_headers,
};

#[cfg(test)]
mod tests;

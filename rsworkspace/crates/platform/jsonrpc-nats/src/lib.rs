//! JSON-RPC 2.0 over NATS codecs.
//!
//! [`encode`] and [`decode`] carry a complete JSON-RPC object in the NATS body,
//! which is authoritative (ADR#0056). `Jsonrpc-Id` and `Jsonrpc-Error-Code` are
//! non-authoritative projections of that body, emitted for routing and metrics.
//!
//! [`encode_value`] and [`decode_value`] are the same wire format over a raw
//! [`serde_json::Value`], preserving the exact member shape a protocol edge sent
//! rather than normalizing it through [`Message`].
#![cfg_attr(test, allow(clippy::expect_used, clippy::panic, clippy::unwrap_used))]

pub mod codec;
pub mod constants;
pub mod direction;
pub mod error;
pub mod id;
pub mod message;
pub mod transport;

pub use codec::{Encoded, decode, decode_value, encode, encode_value, from_json_value, to_json_value};
pub use constants::{HEADER_ERROR_CODE, HEADER_ID, JSONRPC_VERSION};
pub use direction::Direction;
pub use error::CodecError;
pub use id::{
    RequestId, ResponseId, decode_request_id_literal, decode_response_id_literal, encode_id_literal,
    encode_response_id_literal,
};
pub use message::Message;
pub use transport::{
    TransportError, jsonrpc_publish, jsonrpc_publish_with_timeout, jsonrpc_request_raw, jsonrpc_request_with_timeout,
    merge_headers, merge_jsonrpc_headers,
};

#[cfg(test)]
mod tests;

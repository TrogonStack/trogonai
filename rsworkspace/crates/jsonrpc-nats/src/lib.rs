//! JSON-RPC 2.0 over NATS content-mode codec (ADR 0011).
//!
//! Control and correlation fields project to NATS headers; payload stays in the
//! body. Success versus error is decided by the presence of `Jsonrpc-Error-Code`.
//!
//! Reconstruction to and from canonical JSON-RPC happens only at protocol edges
//! (HTTP/SSE, WebSocket, stdio bridges), centralized in this crate — never ad
//! hoc in domain handlers. The NATS backbone carries one content-mode message
//! per NATS message; batch arrays are unbundled at those edges.
#![cfg_attr(test, allow(clippy::expect_used, clippy::panic, clippy::unwrap_used))]

pub mod codec;
pub mod constants;
pub mod direction;
pub mod error;
pub mod id;
pub mod message;
pub mod transport;

pub use codec::{Encoded, decode, encode, from_json_value, to_json_value};
pub use constants::{HEADER_ERROR_CODE, HEADER_ID, JSONRPC_VERSION};
pub use direction::Direction;
pub use error::CodecError;
pub use id::{RequestId, ResponseId, decode_response_id_literal, encode_id_literal, encode_response_id_literal};
pub use message::Message;
pub use transport::{TransportError, jsonrpc_publish, jsonrpc_request_with_timeout, merge_jsonrpc_headers};

#[cfg(test)]
mod tests;

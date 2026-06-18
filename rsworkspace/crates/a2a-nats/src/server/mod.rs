//! Server-side scaffolding for A2A handlers.
//!
//! Operations land in later PRs; this slice ships the wire framing and error
//! surface every handler will reach for.

pub mod handler;
pub mod wire;

pub use handler::{A2aError, TaskEventStream};
pub use wire::{JsonRpcErrorResponse, JsonRpcResponse, parse_request};

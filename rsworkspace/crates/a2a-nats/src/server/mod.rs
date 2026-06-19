//! Server-side scaffolding for A2A handlers.
//!
//! Operations land in later PRs; this slice ships the wire framing and error
//! surface every handler will reach for.

pub mod agent_card;
pub mod handler;
pub mod message_send;
pub mod tasks_cancel;
pub mod tasks_get;
pub mod tasks_list;
pub mod tasks_resubscribe;
#[cfg(test)]
pub mod test_support;
pub mod wire;

pub use handler::{A2aError, A2aExecutor, TaskEventStream};
pub use wire::{JsonRpcErrorResponse, JsonRpcResponse, parse_request};

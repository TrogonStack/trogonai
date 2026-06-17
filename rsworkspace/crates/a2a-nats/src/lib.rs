//! A2A protocol bindings over NATS.
//!
//! Implements the [Agent-to-Agent (A2A) protocol](https://a2a-protocol.org/) over NATS
//! subjects and JetStream streams. This crate is being assembled incrementally;
//! the current slice ships value objects + JSON-RPC codec + protocol constants.

#![cfg_attr(test, allow(clippy::expect_used, clippy::panic, clippy::unwrap_used))]

pub mod a2a_prefix;
pub mod agent_id;
pub mod client;
pub mod constants;
pub mod context_id;
pub mod error;
pub mod jsonrpc;
pub mod req_id;
pub mod server;
pub mod task_id;

pub use a2a_prefix::{A2aPrefix, A2aPrefixError};
pub use agent_id::{A2aAgentId, AgentIdError};
pub use context_id::{A2aContextId, ContextIdError};
pub use error::{AGENT_UNAVAILABLE, TASK_NOT_CANCELABLE, TASK_NOT_FOUND};
pub use jsonrpc::{JsonRpcId, extract_request_id};
pub use req_id::ReqId;
pub use task_id::{A2aTaskId, TaskIdError};

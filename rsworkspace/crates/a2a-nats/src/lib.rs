//! A2A protocol bindings over NATS.
//!
//! Implements the [Agent-to-Agent (A2A) protocol](https://a2a-protocol.org/) over NATS
//! subjects and JetStream streams. This crate is being assembled incrementally;
//! the first slice ships the value-object surface that subjects, dispatch, and
//! handler code will build on.

pub mod a2a_prefix;
pub mod agent_id;
pub mod context_id;
pub mod req_id;
pub mod task_id;

pub use a2a_prefix::{A2aPrefix, A2aPrefixError};
pub use agent_id::{A2aAgentId, AgentIdError};
pub use context_id::{A2aContextId, ContextIdError};
pub use req_id::ReqId;
pub use task_id::{A2aTaskId, TaskIdError};

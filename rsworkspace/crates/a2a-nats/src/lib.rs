//! A2A protocol binding over NATS.
//!
//! Implements the [Agent-to-Agent (A2A) protocol](https://a2a-protocol.org/) over NATS
//! subjects and JetStream streams. Mirrors the patterns established by `acp-nats` and
//! `mcp-nats` for JSON-RPC-over-NATS protocols in this workspace.
//!
//! See `A2A_PLAN.md` at the repo root for the architecture rationale (subject layout,
//! discovery via KV, JetStream-backed streaming, push notification mapping).

pub mod a2a_prefix;
pub mod agent;
pub mod agent_id;
pub mod audit;
pub mod catalog;
pub mod client;
pub mod config;
pub mod constants;
pub mod context_id;
pub mod error;
pub mod jetstream;
pub mod jsonrpc;
pub mod nats;
pub mod push;
pub mod req_id;
pub mod task_id;

pub use a2a_prefix::{A2aPrefix, A2aPrefixError};
pub use agent_id::{A2aAgentId, AgentIdError};
pub use config::{Config, DEFAULT_A2A_PREFIX, ENV_A2A_PREFIX, apply_timeout_overrides, nats_connect_timeout};
pub use context_id::{A2aContextId, ContextIdError};
pub use error::{AGENT_UNAVAILABLE, TASK_NOT_CANCELABLE, TASK_NOT_FOUND};
pub use jsonrpc::{JsonRpcId, extract_request_id};
pub use req_id::ReqId;
pub use task_id::{A2aTaskId, TaskIdError};
pub use trogon_nats::{NatsAuth, NatsConfig};

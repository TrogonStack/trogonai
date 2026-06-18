//! A2A protocol bindings over NATS.
//!
//! Implements the [Agent-to-Agent (A2A) protocol](https://a2a-protocol.org/) over NATS
//! subjects and JetStream streams. This crate is being assembled incrementally;
//! the current slice ships value objects + JSON-RPC codec + protocol constants.

#![cfg_attr(test, allow(clippy::expect_used, clippy::panic, clippy::unwrap_used))]

pub mod a2a_prefix;
pub mod agent_id;
pub mod audit;
pub mod catalog;
pub mod client;
pub mod constants;
pub mod context_id;
pub mod error;
pub mod gateway_ingress;
pub mod jetstream;
pub mod jsonrpc;
pub mod nats;
pub mod push;
pub mod req_id;
pub mod server;
pub mod task_id;

pub use a2a_prefix::{A2aPrefix, A2aPrefixError};
pub use agent_id::{A2aAgentId, AgentIdError};
pub use context_id::{A2aContextId, ContextIdError};
pub use error::{AGENT_UNAVAILABLE, TASK_NOT_CANCELABLE, TASK_NOT_FOUND};
pub use gateway_ingress::{
    GATEWAY_INGRESS_METHOD_SUFFIXES, GatewayComposeError, GatewayIngressError, compose_gateway_ingress_subject,
    gateway_ingress_agent_and_method_dots, gateway_ingress_subject_from_agent_subject,
    ingress_gateway_deadline_exceeded_response_bytes, ingress_gateway_declarative_denied_response_bytes,
    ingress_gateway_policy_denied_response_bytes, ingress_gateway_tier3_refused_response_bytes,
    ingress_invalid_request_response_bytes, resolve_gateway_ingress_subject,
};
pub use jsonrpc::{JsonRpcId, extract_request_id};
pub use req_id::ReqId;
pub use task_id::{A2aTaskId, TaskIdError};

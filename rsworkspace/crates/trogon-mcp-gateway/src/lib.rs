//! MCP gateway queue-group worker: ingress `{prefix}.gateway.request.*`, egress `{prefix}.server.*`.

pub mod act_chain;
pub mod agent_identity;
pub mod audit;
pub mod authz;
pub mod gateway;
pub mod jwt;
pub mod policy;
pub mod rpc_codes;
pub mod spicedb;
pub mod subject;
pub mod trace;

pub use gateway::{GatewayError, run};

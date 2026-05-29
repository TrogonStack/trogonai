//! MCP gateway queue-group worker: ingress `{prefix}.gateway.request.*`, egress `{prefix}.server.*`.

pub mod act_chain;
pub mod agent_identity;
pub mod anomaly;
pub mod approvals;
pub mod audit;
pub mod authz;
pub mod cel_builtins;
pub mod egress;
pub mod gateway;
pub mod ingress;
pub mod jwt;
pub mod policy;
pub mod redaction;
pub mod rpc_codes;
pub mod schema_cache;
pub mod throttle;
pub mod spicedb;
pub mod subject;
pub mod trace;
pub mod wasm;

pub use gateway::{GatewayError, run};

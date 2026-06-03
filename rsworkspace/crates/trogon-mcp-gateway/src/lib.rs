//! MCP gateway queue-group worker: ingress `{prefix}.gateway.request.*`, egress `{prefix}.server.*`.

pub mod aauth;
pub mod act_chain;
pub mod agent_identity;
pub mod anomaly;
pub mod approvals;
pub mod audience_shadow;
pub mod audit;
pub mod authz;
pub mod bundle;
pub mod cel_builtins;
pub mod chain_resolver;
pub mod context_throttle;
pub mod egress;
pub mod gateway;
pub mod health;
pub mod ingress;
pub mod jwt;
pub mod multi_region;
pub mod observability;
pub mod plugin;
pub mod policy;
pub mod redaction;
pub mod rpc_codes;
pub mod schema_cache;
pub mod spicedb;
pub mod stepup;
pub mod subject;
pub mod throttle;
pub mod trace;
pub mod wasm;

pub use gateway::{GatewayError, run};

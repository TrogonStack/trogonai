//! A2A gateway service — ingress on `{prefix}.gateway.>` and forward to `{prefix}.agent.{id}.{method}`.
//!
//! Engineering checklist beyond opaque forward: **[`docs/a2a/explanation/gateway-roadmap.md`](../../../../docs/a2a/explanation/gateway-roadmap.md)**.
//!
//! Planned authorization and policy seams are documented in [A2A architecture](../../../../docs/a2a/explanation/architecture.md):
//! queue-group subscriber (`A2A_GATEWAY_QUEUE_GROUP`) with opaque JSON-RPC bridging today; JWT
//! validation, policy bundles (`a2a-pack`), and ingress audit emission remain future work.
//!
//! ## Future: authenticated caller identity
//!
//! The gateway will propagate authenticated caller identity from minted NATS User JWTs (auth-callout)
//! into request handling for correlation and audit enrichment. Ingress spans already reserve a
//! `caller_id` field for this JWT-derived identity once extraction is wired.
//!
//! If the gateway later owns push remediation, DLQ subjects may include caller segments such as
//! `{prefix}.push.dlq.{caller_id}.{task_id}`. Terminal push DLQ publishes today originate from
//! the `a2a-nats` agent `Bridge` / `message/stream` pump — not from this gateway forwarding layer.

pub mod agent_card_surface;
pub mod caller_jwt_header;
pub mod config;
pub mod gw_ingress_stream;
pub mod gw_pull_backpressure;
pub mod jwt_caller_identity;
pub mod policy;
pub mod push_dlq_mirror;
pub mod runtime;

pub use config::{Args, Config, ConfigError};
pub use runtime::RuntimeError;

use trogon_std::env::SystemEnv;

pub async fn run(args: Args) -> Result<(), RuntimeError> {
    runtime::run_with_args(args, &SystemEnv).await
}

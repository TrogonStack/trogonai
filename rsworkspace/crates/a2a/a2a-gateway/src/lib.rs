//! A2A gateway service.
//!
//! Slice g2 wires the env-driven [`Config`] + [`Args`] CLI surface and a
//! [`runtime`] entry point that subsequent slices flesh out with audit,
//! ingress streaming, and policy execution. The boot path is `main.rs` →
//! [`run`] → [`runtime::run_with_args`] so the binary and integration tests
//! share a single seam.
//!
//! Modules:
//! - [`aauth`] — AAuth (draft-hardt-aauth-protocol) ingress verifier; turns
//!   inline `aa-agent+jwt` + PoP + optional `aa-auth+jwt` headers into an
//!   [`aauth::AAuthResolution`] or an [`aauth::AAuthDeny`] carrying a
//!   `ResourceChallenge` for the reply.
//! - [`agent_card_surface`] — schema-validates AgentCard JSON before the
//!   gateway's discover surface returns it, so a stored card that drifted
//!   from the spec can't be surfaced unchecked.
//! - [`caller_jwt_header`] — re-exports the wire-level header constants from
//!   `a2a-auth-callout` so callers only depend on this crate.
//! - [`config`] — clap-derived [`Args`] + env-resolved [`Config`].
//! - [`gw_ingress_stream`] — gateway-owned JetStream pull pipe for
//!   streaming ingress (`message/stream`, `tasks/resubscribe`).
//! - [`gw_pull_backpressure`] — pull consumer for task-event egress
//!   (`A2A_EVENTS`) with JetStream flow control + per-caller inflight cap.
//! - [`jwt_caller_identity`] — resolves a verified caller identity from a
//!   minted NATS User JWT carried on the inbound message, with a
//!   labs-only header-trust fallback gated behind an env flag.
//! - [`policy`] — shared policy-tier scaffold; later slices layer Tier 1
//!   declarative, Tier 1 SpiceDB, Tier 2 CEL, and Tier 3 redaction on top.
//! - [`push_dlq_mirror`] — pull-consumer that mirrors `{prefix}.push.dlq.>`
//!   into a tenant-readable `mirror.*` view with in-process dedupe so a
//!   re-delivered DLQ envelope only publishes once.
//! - [`runtime`] — boot orchestration; surfaces [`RuntimeError`] as the
//!   terminal error for the `main` binary.

#![allow(clippy::module_name_repetitions)]
#![cfg_attr(test, allow(clippy::expect_used, clippy::panic, clippy::unwrap_used))]

pub mod aauth;
pub mod agent_card_surface;
pub mod audit_ingress;
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

/// Boot entrypoint: parse env + run the gateway runtime. Returns the
/// runtime's terminal error if one fires. The binary calls this directly so
/// `main.rs` stays a thin shim and integration tests can inject a fake env
/// through [`runtime::run_with_args`].
pub async fn run(args: Args) -> Result<(), RuntimeError> {
    runtime::run_with_args(args, &trogon_std::env::SystemEnv).await
}

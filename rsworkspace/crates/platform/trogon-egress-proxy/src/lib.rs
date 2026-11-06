#![cfg_attr(test, allow(clippy::expect_used, clippy::panic, clippy::unwrap_used))]

//! Agent egress proxy: an mTLS HTTP CONNECT forward proxy that tunnels only to
//! the exact destinations a policy snapshot permits for the calling agent
//! (ADR#0085).
//!
//! The decision logic is pure and lives apart from the sockets:
//! [`admission::admit`] answers from a [`policy_snapshot::PolicySnapshot`],
//! [`dial_targets::dial_targets`] filters resolved addresses, and
//! [`client_hello::server_name`] reads the SNI. [`connection::Proxy`] sequences
//! them and [`server::Server`] owns the listener. Every decision is recorded
//! through [`writer_audit::WriterAudit`] into the platform audit stream, and a
//! permit that cannot be recorded is refused rather than forwarded.

pub mod address_class;
pub mod admission;
pub mod agent_id;
pub mod agent_uri;
pub mod audit;
pub mod audit_config;
pub mod audit_error;
pub mod client_hello;
pub mod config;
pub mod connect_request;
pub mod connection;
mod constants;
mod decision_record;
pub mod destination;
pub mod destination_host;
pub mod dial_targets;
pub mod fault;
pub mod first_record;
pub mod host_name;
pub mod limits;
pub mod mtls;
pub mod peer_certificate;
pub mod peer_identity;
pub mod policy_snapshot;
pub mod policy_source;
pub mod resolver;
pub mod runtime;
pub mod server;
pub mod static_snapshot;
pub mod trust_domain;
pub mod writer_audit;

pub use agent_id::AgentId;
pub use config::{Args, Config};
pub use runtime::{StartupError, start};
pub use server::Server;

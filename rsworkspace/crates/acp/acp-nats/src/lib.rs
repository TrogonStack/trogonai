#![cfg_attr(test, allow(clippy::expect_used, clippy::panic, clippy::unwrap_used))]

pub mod acp_prefix;
pub mod agent;
pub mod agent_handler;
pub mod boundary;
pub mod client;
pub mod client_handler;
pub mod client_proxy;
pub mod config;
pub mod constants;
pub mod error;
pub mod ext_method_name;
pub(crate) mod in_flight_slot_guard;
pub mod jetstream;
pub mod nats;
pub mod req_id;
pub mod session_id;
pub(crate) mod telemetry;
pub mod wire;

pub use acp_prefix::{AcpPrefix, AcpPrefixError};
pub use agent::Bridge;
pub use agent_handler::AgentHandler;
pub use client_handler::ClientHandler;
pub use client_proxy::NatsClientProxy;
pub use config::{Config, DEFAULT_ACP_PREFIX, ENV_ACP_PREFIX, apply_timeout_overrides, nats_connect_timeout};
pub use error::AGENT_UNAVAILABLE;
pub use ext_method_name::ExtMethodName;
pub use nats::responses::ResponseSubject;
pub use nats::{FlushClient, PublishClient, RequestClient, SubscribeClient};
pub use req_id::ReqId;
pub use session_id::AcpSessionId;
#[cfg(not(coverage))]
pub use trogon_nats::jetstream::NatsJetStreamClient;
pub use trogon_nats::jetstream::{JetStreamGetStream, JetStreamPublisher};
pub use trogon_nats::{NatsAuth, NatsConfig};
pub use trogon_std::StdJsonSerialize;

#[cfg(test)]
mod tests;

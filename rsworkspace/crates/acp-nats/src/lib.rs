#![cfg_attr(coverage, feature(coverage_attribute))]

pub mod acp_prefix;
pub mod agent;
pub mod client;
pub mod config;
pub mod error;
pub(crate) mod ext_method_name;
pub(crate) mod in_flight_slot_guard;
pub(crate) mod jsonrpc;
pub mod nats;
pub(crate) mod pending_prompt_waiters;
pub mod prompt_event;
pub(crate) mod prompt_slot_counter;
pub mod session_id;
pub mod subject_token_violation;
pub(crate) mod telemetry;

pub use acp_prefix::{AcpPrefix, AcpPrefixError};
pub use agent::Bridge;
pub use config::{
    Config, DEFAULT_ACP_PREFIX, ENV_ACP_PREFIX, apply_timeout_overrides, nats_connect_timeout,
};
pub use error::AGENT_UNAVAILABLE;
pub use nats::{FlushClient, PublishClient, RequestClient, SubscribeClient};
pub use prompt_event::{PromptEvent, PromptPayload};
pub use session_id::AcpSessionId;
pub use trogon_nats::{NatsAuth, NatsConfig};
pub use trogon_std::StdJsonSerialize;

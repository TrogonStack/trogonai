pub mod agent;
pub mod client;
pub mod config;
pub mod error;
pub mod ext_method_name;
pub mod nats;
pub mod session_id;
pub(crate) mod telemetry;

#[cfg(test)]
mod tests;

pub use agent::Bridge;
pub use config::{AcpPrefix, Config, ValidationError, validate_method_name, validate_prefix};
pub use error::AGENT_UNAVAILABLE;
pub use ext_method_name::ExtMethodName;
pub use nats::{FlushClient, PublishClient, RequestClient, SubscribeClient};
pub use session_id::AcpSessionId;
pub use trogon_nats::{NatsAuth, NatsConfig};

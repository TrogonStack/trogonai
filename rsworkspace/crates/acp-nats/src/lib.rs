pub mod agent;
pub mod client;
pub mod config;
pub(crate) mod metrics;
pub mod nats;

#[cfg(test)]
mod tests;

pub use agent::Bridge;
pub use config::Config;
pub use nats::{FlushClient, PublishClient, RequestClient, SubscribeClient};
pub use trogon_nats::{NatsAuth, NatsConfig};

pub(crate) const JSONRPC_INTERNAL_ERROR: i32 = -32603;
pub(crate) const JSONRPC_SERVER_ERROR: i32 = -32000;

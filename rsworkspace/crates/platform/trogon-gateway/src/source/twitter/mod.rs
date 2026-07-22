//! Twitter/X webhook receiver that validates CRC and signed deliveries, then
//! publishes verified payloads to NATS JetStream.

pub mod config;
pub mod constants;
pub mod server;
pub mod signature;

pub use config::TwitterConfig;
pub use server::{provision, router};

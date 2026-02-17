//! NATS messaging infrastructure for Discord integration

pub mod config;
pub mod error;
pub mod messaging;
pub mod nats;
pub mod subjects;

pub use config::NatsConfig;
pub use error::{Error, Result};
pub use messaging::{MessagePublisher, MessageSubscriber};
pub use nats::connect;

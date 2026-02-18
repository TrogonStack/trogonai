//! NATS messaging infrastructure for Telegram integration
//!
//! This crate provides the core messaging layer for the Telegram bot bridge,
//! including subject patterns, publish/subscribe helpers, connection management,
//! and observability integration.

pub mod config;
pub mod dedup;
pub mod error;
pub mod messaging;
pub mod nats;
pub mod subjects;

// Re-export commonly used types
pub use config::{NatsConfig, TelegramConfig};
pub use dedup::DedupStore;
pub use error::{Error, Result};
pub use messaging::{MessagePublisher, MessageSubscriber};
pub use nats::connect;

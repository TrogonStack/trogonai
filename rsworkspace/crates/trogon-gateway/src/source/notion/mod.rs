//! # trogon-source-notion
//!
//! Notion webhook receiver that publishes subscription verification requests
//! and verified events to NATS JetStream.

pub mod config;
pub mod constants;
pub mod notion_event_type;
pub mod notion_verification_token;
pub mod server;
pub mod signature;
pub mod verification_token;

pub use config::NotionConfig;
pub use notion_event_type::NotionEventType;
pub use notion_verification_token::NotionVerificationToken;
pub use server::{provision, router};

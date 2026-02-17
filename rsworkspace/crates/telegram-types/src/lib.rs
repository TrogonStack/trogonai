//! Shared types for Telegram integration with NATS
//!
//! This crate provides common types used across the Telegram bot bridge
//! and agent implementations, including events, commands, chat types,
//! policies, and session management.

pub mod chat;
pub mod commands;
pub mod errors;
pub mod events;
pub mod policies;
pub mod session;

// Re-export commonly used types
pub use chat::{Chat, ChatType, Message, User};
pub use commands::*;
pub use errors::{CommandErrorEvent, ErrorCategory, TelegramErrorCode};
pub use events::*;
pub use policies::{AccessConfig, DmPolicy, GroupPolicy};
pub use session::SessionId;

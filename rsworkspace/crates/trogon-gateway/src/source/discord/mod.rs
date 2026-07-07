//! # trogon-source-discord
//!
//! Inbound pipe for Discord gateway events into NATS JetStream.
//!
//! Connects to Discord via the Gateway WebSocket using twilight-gateway.
//! Every gateway event is serialized to JSON and published to NATS on
//! `{prefix}.{event_name}` subjects (e.g. `discord.message_create`,
//! `discord.guild_member_add`). No filtering, no access control: dumb pipe.
//!
//! The bot token can come from static config or the runtime credential registry.

pub mod config;
pub mod constants;
pub mod gateway;
#[cfg(not(coverage))]
pub mod gateway_runner;

pub use config::DiscordConfig;
pub use gateway::provision;

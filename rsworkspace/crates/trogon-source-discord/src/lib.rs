//! # trogon-source-discord
//!
//! Inbound pipe for Discord events into NATS JetStream. Runs in one of two
//! mutually exclusive modes controlled by `DISCORD_MODE`:
//!
//! ## Gateway mode (`DISCORD_MODE=gateway`)
//!
//! Connects to Discord via the Gateway WebSocket using twilight-gateway.
//! Every gateway event is serialized to JSON and published to NATS on
//! `{prefix}.{event_name}` subjects (e.g. `discord.message_create`,
//! `discord.guild_member_add`). No filtering, no access control — dumb pipe.
//!
//! Requires `DISCORD_BOT_TOKEN`.
//!
//! ## Webhook mode (`DISCORD_MODE=webhook`)
//!
//! Discord sends `POST /webhook` with `X-Signature-Ed25519` and
//! `X-Signature-Timestamp` headers plus a JSON interaction payload.
//! The server validates the Ed25519 signature against `DISCORD_PUBLIC_KEY`.
//! PING interactions (type 1) are answered inline with `{"type":1}`.
//! All other interactions are published to NATS JetStream on
//! `{prefix}.{interaction_type}` subjects. Only receives interactions (slash
//! commands, buttons, modals, autocomplete).
//!
//! Requires `DISCORD_PUBLIC_KEY`.

pub mod config;
pub mod constants;
pub mod gateway;
pub mod server;
pub mod signature;

pub use config::{DiscordConfig, SourceMode};
#[cfg(not(coverage))]
pub use server::{ServeError, serve};
pub use server::{provision, router};
pub use signature::SignatureError;

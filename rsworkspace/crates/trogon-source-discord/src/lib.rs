//! # trogon-source-discord
//!
//! Discord interaction webhook receiver that publishes events to NATS JetStream.
//!
//! ## How it works
//!
//! 1. Discord sends `POST /webhook` with `X-Signature-Ed25519` and
//!    `X-Signature-Timestamp` headers plus a JSON interaction payload.
//! 2. The server validates the Ed25519 signature against `DISCORD_PUBLIC_KEY`.
//! 3. PING interactions (type 1) are answered inline with `{"type":1}`.
//! 4. All other interactions are published to NATS JetStream on
//!    `discord.{interaction_type}` subjects (e.g. `discord.application_command`).
//! 5. The JetStream stream (`DISCORD` by default, capturing `discord.>`) is
//!    created automatically on startup if it doesn't exist.
//!
//! ## NATS message format
//!
//! - **Subject**: `{DISCORD_SUBJECT_PREFIX}.{interaction_type}`
//!   (e.g. `discord.application_command`, `discord.message_component`)
//! - **Headers**: `X-Discord-Interaction-Type`, `X-Discord-Interaction-Id`
//! - **Payload**: raw JSON body from Discord
//!
//! ## Configuration (env vars)
//!
//! | Variable | Default | Description |
//! |---|---|---|
//! | `DISCORD_PUBLIC_KEY` | **required** | Ed25519 public key (hex) from Discord app settings |
//! | `DISCORD_WEBHOOK_PORT` | `8080` | HTTP listening port |
//! | `DISCORD_SUBJECT_PREFIX` | `discord` | NATS subject prefix |
//! | `DISCORD_STREAM_NAME` | `DISCORD` | JetStream stream name |
//! | `DISCORD_STREAM_MAX_AGE_SECS` | `604800` | Max age of messages in JetStream (seconds, default 7 days) |
//! | `DISCORD_NATS_ACK_TIMEOUT_SECS` | `10` | NATS publish ack timeout in seconds |
//! | `DISCORD_MAX_BODY_SIZE` | `4194304` | Maximum webhook body size in bytes (default 4 MB) |
//! | `NATS_URL` | `localhost:4222` | NATS server URL(s) |

pub mod config;
pub mod constants;
pub mod server;
pub mod signature;

pub use config::DiscordConfig;
#[cfg(not(coverage))]
pub use server::{ServeError, serve};
pub use server::{provision, router};
pub use signature::SignatureError;

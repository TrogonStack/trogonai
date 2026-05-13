//! # trogon-source-telegram
//!
//! Telegram webhook receiver that publishes updates to NATS JetStream.
//!
//! ## How it works
//!
//! 1. Telegram sends `POST /webhook` with `X-Telegram-Bot-Api-Secret-Token`
//!    header plus a JSON `Update` payload.
//! 2. The server validates the secret token against `TELEGRAM_WEBHOOK_SECRET`.
//! 3. Updates are published to NATS JetStream on `telegram.{update_type}` subjects
//!    (e.g. `telegram.message`, `telegram.callback_query`).
//! 4. The JetStream stream (`TELEGRAM` by default, capturing `telegram.>`) is
//!    created automatically on startup if it doesn't exist.
//!
//! ## NATS message format
//!
//! - **Subject**: `{TELEGRAM_SUBJECT_PREFIX}.{update_type}` (e.g. `telegram.message`)
//! - **Headers**: `X-Telegram-Update-Type`, `X-Telegram-Update-Id`
//! - **Payload**: raw JSON body from Telegram
//!
//! ## Configuration (env vars)
//!
//! | Variable | Default | Description |
//! |---|---|---|
//! | `TELEGRAM_WEBHOOK_SECRET` | **required** | Secret token configured via `setWebhook` |
//! | `TELEGRAM_WEBHOOK_REGISTRATION_MODE` | `manual` | Use `startup` to register the webhook on startup |
//! | `TELEGRAM_BOT_TOKEN` | — | Bot token used to register the webhook on startup |
//! | `TELEGRAM_PUBLIC_WEBHOOK_URL` | — | Public HTTPS URL registered with Telegram |
//! | `TELEGRAM_SOURCE_PORT` | `8080` | HTTP listening port |
//! | `TELEGRAM_SUBJECT_PREFIX` | `telegram` | NATS subject prefix |
//! | `TELEGRAM_STREAM_NAME` | `TELEGRAM` | JetStream stream name |
//! | `TELEGRAM_STREAM_MAX_AGE_SECS` | `604800` | Max age of messages in JetStream (seconds, default 7 days) |
//! | `TELEGRAM_NATS_ACK_TIMEOUT_SECS` | `10` | NATS publish ack timeout in seconds |
//! | `TELEGRAM_MAX_BODY_SIZE` | `10485760` | Maximum webhook body size in bytes (default 10 MB) |
//! | `NATS_URL` | `localhost:4222` | NATS server URL(s) |

pub mod config;
pub mod constants;
pub mod registration;
pub mod server;
pub mod signature;

pub use config::TelegramSourceConfig;
pub use server::{provision, router};

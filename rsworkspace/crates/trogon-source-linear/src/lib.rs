//! # trogon-source-linear
//!
//! Linear webhook receiver that publishes events to NATS JetStream.
//!
//! ## How it works
//!
//! 1. Linear sends `POST /webhook` with a `linear-signature` header plus a JSON payload.
//! 2. The server validates the HMAC-SHA256 signature against `LINEAR_WEBHOOK_SECRET`.
//! 3. Events are published to NATS JetStream on `{prefix}.{type}.{action}` subjects
//!    (e.g. `linear.Issue.create`, `linear.Comment.update`).
//! 4. The JetStream stream (`LINEAR` by default, capturing `linear.>`) is created
//!    automatically on startup if it doesn't exist.
//!
//! ## NATS message format
//!
//! - **Subject**: `{LINEAR_SUBJECT_PREFIX}.{type}.{action}` (e.g. `linear.Issue.create`)
//! - **Headers**: `Nats-Msg-Id` (set to Linear's `webhookId` for dedup)
//! - **Payload**: raw JSON body from Linear
//!
//! ## Configuration (env vars)
//!
//! | Variable | Default | Description |
//! |---|---|---|
//! | `LINEAR_WEBHOOK_SECRET` | — | Signing secret from Linear's webhook settings (required) |
//! | `LINEAR_WEBHOOK_PORT` | `8080` | HTTP listening port |
//! | `LINEAR_SUBJECT_PREFIX` | `linear` | NATS subject prefix |
//! | `LINEAR_STREAM_NAME` | `LINEAR` | JetStream stream name |
//! | `LINEAR_STREAM_MAX_AGE_SECS` | `604800` | Max age of messages in JetStream (seconds, default 7 days) |
//! | `LINEAR_WEBHOOK_TIMESTAMP_TOLERANCE_SECS` | `60` | Replay-attack window in seconds (set to 0 to disable) |
//! | `LINEAR_NATS_ACK_TIMEOUT_MS` | `10000` | How long to wait for a JetStream ACK (milliseconds) |
//! | `NATS_URL` | `localhost:4222` | NATS server URL(s) |

pub mod config;
pub mod constants;
pub mod server;
pub mod signature;

pub use config::LinearConfig;
#[cfg(not(coverage))]
pub use server::serve;

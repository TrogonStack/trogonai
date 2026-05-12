//! # trogon-source-slack
//!
//! Slack Events API webhook receiver that publishes events to NATS JetStream.
//!
//! ## How it works
//!
//! 1. Slack sends `POST /webhook` with `X-Slack-Signature` and
//!    `X-Slack-Request-Timestamp` headers plus a JSON payload.
//! 2. The server validates the HMAC-SHA256 signature against `SLACK_SIGNING_SECRET`.
//! 3. `url_verification` challenges are answered inline (no NATS publish).
//! 4. `event_callback` payloads are published to NATS JetStream on
//!    `slack.{event.type}` subjects (e.g. `slack.message`, `slack.app_mention`).
//! 5. The JetStream stream (`SLACK` by default, capturing `slack.>`) is created
//!    automatically on startup if it doesn't exist.
//!
//! ## NATS message format
//!
//! - **Subject**: `{SLACK_SUBJECT_PREFIX}.{event.type}` (e.g. `slack.message`)
//! - **Headers**: `X-Slack-Event-Type`, `X-Slack-Event-Id`, `X-Slack-Team-Id`
//! - **Payload**: raw JSON body from Slack
//!
//! ## Configuration (env vars)
//!
//! | Variable | Default | Description |
//! |---|---|---|
//! | `SLACK_SIGNING_SECRET` | **required** | Slack app signing secret |
//! | `SLACK_WEBHOOK_PORT` | `3000` | HTTP listening port |
//! | `SLACK_SUBJECT_PREFIX` | `slack` | NATS subject prefix |
//! | `SLACK_STREAM_NAME` | `SLACK` | JetStream stream name |
//! | `SLACK_STREAM_MAX_AGE_SECS` | `604800` | Max age of messages in JetStream (seconds, default 7 days) |
//! | `SLACK_NATS_ACK_TIMEOUT_SECS` | `10` | NATS publish ack timeout in seconds |
//! | `SLACK_MAX_BODY_SIZE` | `1048576` | Maximum webhook body size in bytes (default 1 MB) |
//! | `SLACK_TIMESTAMP_MAX_DRIFT_SECS` | `300` | Max clock drift for request timestamps (default 5 min) |
//! | `NATS_URL` | `localhost:4222` | NATS server URL(s) |

pub mod config;
pub mod constants;
pub mod server;
pub mod signature;

pub use config::SlackConfig;
pub use server::{provision, router};

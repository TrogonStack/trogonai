//! # trogon-datadog
//!
//! Datadog webhook receiver that publishes events to NATS JetStream.
//!
//! ## How it works
//!
//! 1. Datadog sends `POST /webhook` with `X-Datadog-Signature` and
//!    `DD-Request-ID` headers plus a JSON payload.
//! 2. The server validates the HMAC-SHA256 signature against `DATADOG_WEBHOOK_SECRET`.
//! 3. Events are published to NATS JetStream on `datadog.{event_type}` subjects
//!    (e.g. `datadog.alert`, `datadog.alert.recovered`).
//! 4. The JetStream stream (`DATADOG` by default, capturing `datadog.>`) is created
//!    automatically on startup if it doesn't exist.
//!
//! ## NATS message format
//!
//! - **Subject**: `{DATADOG_SUBJECT_PREFIX}.{event_type}` (derived from payload)
//! - **Headers**: `DD-Request-ID`, `X-Datadog-Event-Type`
//! - **Payload**: raw JSON body from Datadog
//!
//! ## Configuration (env vars)
//!
//! | Variable | Default | Description |
//! |---|---|---|
//! | `DATADOG_WEBHOOK_SECRET` | — | HMAC-SHA256 secret (omit to skip validation) |
//! | `DATADOG_WEBHOOK_PORT` | `8080` | HTTP listening port |
//! | `DATADOG_SUBJECT_PREFIX` | `datadog` | NATS subject prefix |
//! | `DATADOG_STREAM_NAME` | `DATADOG` | JetStream stream name |
//! | `DATADOG_STREAM_MAX_AGE_SECS` | `604800` | Max age of messages in JetStream (seconds, default 7 days) |
//! | `NATS_URL` | `localhost:4222` | NATS server URL(s) |

pub mod config;
pub mod server;
pub mod signature;

pub use config::DatadogConfig;
pub use server::serve;

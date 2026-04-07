//! # trogon-incidentio
//!
//! incident.io webhook receiver that publishes events to NATS JetStream and
//! maintains a live incident state snapshot in a NATS KV bucket.
//!
//! ## How it works
//!
//! 1. incident.io sends `POST /webhook` with `X-Incident-Signature` and
//!    `X-Incident-Delivery` headers plus a JSON payload.
//! 2. The server validates the HMAC-SHA256 signature against `INCIDENTIO_WEBHOOK_SECRET`.
//! 3. The incident state is upserted into the `INCIDENTS` KV bucket (keyed by incident ID).
//! 4. The raw payload is published to NATS JetStream on `incidentio.{event_type}` subjects
//!    (e.g. `incidentio.incident.created`, `incidentio.incident.resolved`).
//!
//! ## NATS message format
//!
//! - **Subject**: `{INCIDENTIO_SUBJECT_PREFIX}.{event_type}`
//! - **Headers**: `X-Incident-Event-Type`, `X-Incident-Delivery`
//! - **Payload**: raw JSON body from incident.io
//!
//! ## Configuration (env vars)
//!
//! | Variable | Default | Description |
//! |---|---|---|
//! | `INCIDENTIO_WEBHOOK_SECRET` | — | HMAC-SHA256 signing secret (omit to skip validation) |
//! | `INCIDENTIO_API_TOKEN` | — | Bearer token for the incident.io REST API |
//! | `INCIDENTIO_WEBHOOK_PORT` | `8081` | HTTP listening port |
//! | `INCIDENTIO_SUBJECT_PREFIX` | `incidentio` | NATS subject prefix |
//! | `INCIDENTIO_STREAM_NAME` | `INCIDENTIO` | JetStream stream name |
//! | `INCIDENTIO_STREAM_MAX_AGE_SECS` | `604800` | Max age of messages in JetStream (7 days) |
//! | `NATS_URL` | `localhost:4222` | NATS server URL(s) |

pub mod client;
pub mod config;
pub mod events;
pub mod server;
pub mod signature;
pub mod store;

pub use client::IncidentioClient;
pub use config::IncidentioConfig;
#[cfg(not(coverage))]
pub use server::serve;
pub use store::IncidentStore;

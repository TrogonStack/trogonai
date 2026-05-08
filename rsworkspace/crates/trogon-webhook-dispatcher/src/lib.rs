//! # trogon-webhook-dispatcher
//!
//! Outbound webhook dispatcher for the trogon platform.
//!
//! ## How it works
//!
//! 1. Subscriptions are registered via `POST /subscriptions` on the management
//!    HTTP API, specifying a NATS subject pattern and a target URL.
//! 2. Subscriptions are stored in a NATS KV bucket (`WEBHOOK_SUBSCRIPTIONS`).
//! 3. A durable JetStream pull consumer watches a configured stream (default:
//!    `TRANSCRIPTS`, subject filter: `transcripts.>`).
//! 4. For each message, the dispatcher finds all subscriptions whose pattern
//!    matches the message subject and POSTs the payload to the configured URL.
//! 5. Outbound requests carry `X-Trogon-Event`, `X-Trogon-Delivery`, and
//!    optionally `X-Trogon-Signature-256` (HMAC-SHA256, same format as GitHub).
//!
//! ## Configuration (env vars)
//!
//! | Variable                        | Default              | Description                                   |
//! |---------------------------------|----------------------|-----------------------------------------------|
//! | `WEBHOOK_STREAM_NAME`           | `TRANSCRIPTS`        | JetStream stream to subscribe to              |
//! | `WEBHOOK_SUBJECT_FILTER`        | `transcripts.>`      | Subject filter for the durable consumer       |
//! | `WEBHOOK_CONSUMER_NAME`         | `webhook-dispatcher` | Durable consumer name                         |
//! | `WEBHOOK_PORT`                  | `8080`               | Management HTTP API port                      |
//! | `WEBHOOK_DISPATCH_TIMEOUT_SECS` | `30`                 | Per-request timeout for outbound deliveries   |
//! | Standard `NATS_*` variables for NATS connection (see `trogon-nats`)

pub mod config;
pub mod dispatcher;
pub mod http_client;
pub mod pattern;
pub mod provision;
pub mod registry;
pub mod server;
pub mod signature;
pub mod store;
pub mod subscription;

pub use config::WebhookDispatcherConfig;
pub use dispatcher::Dispatcher;
pub use http_client::ReqwestWebhookClient;
pub use provision::provision;
pub use registry::WebhookRegistry;
#[cfg(not(coverage))]
pub use server::serve;
pub use store::SubscriptionStore;
pub use subscription::WebhookSubscription;

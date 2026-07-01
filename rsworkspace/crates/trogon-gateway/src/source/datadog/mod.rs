//! Datadog webhook receiver that publishes verified events to NATS JetStream.
//!
//! Datadog webhooks are not signed. Requests are authenticated with a shared
//! secret token the operator configures as a custom header on the Datadog
//! webhook, and the payload is an operator-defined JSON template. Routing keys
//! off an `event_type` field in the body, falling back to `.unroutable` when it
//! is absent or invalid.

pub mod config;
pub mod constants;
pub mod datadog_event_type;
pub mod datadog_webhook_token;
pub mod server;
pub mod signature;

pub use config::DatadogConfig;
pub use datadog_event_type::DatadogEventType;
pub use datadog_webhook_token::DatadogWebhookToken;
pub use server::{provision, router};

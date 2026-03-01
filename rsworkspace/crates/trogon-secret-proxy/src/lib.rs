//! HTTP proxy + NATS JetStream worker for AI provider token exchange.
//!
//! # Architecture
//!
//! ```text
//! Service → POST http://proxy:8080/anthropic/v1/messages
//!           Authorization: Bearer tok_anthropic_prod_abc
//!               ↓
//!          [HTTP Proxy (axum)]
//!               ↓ publishes OutboundHttpRequest to JetStream "PROXY_REQUESTS"
//!               ↓ subscribes to reply subject (Core NATS): {prefix}.proxy.reply.{uuid}
//!               ↓ waits up to 60s
//!          [Detokenization Worker (JetStream pull consumer)]
//!               ↓ resolves tok_... → real key via VaultStore
//!               ↓ calls upstream AI provider with real key
//!               ↓ publishes OutboundHttpResponse to reply subject (Core NATS)
//!          [HTTP Proxy receives reply, returns HTTP response to service]
//! ```

pub mod config;
pub mod messages;
pub mod provider;
pub mod proxy;
pub mod stream;
pub mod subjects;
pub mod vault_admin;
pub mod worker;

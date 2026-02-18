//! # trogon-nats
//!
//! Shared NATS infrastructure for TrogonStack applications.
//!
//! This crate provides:
//! - Per-operation NATS client traits for testability (zero-cost via monomorphization)
//! - Connection management with automatic reconnection
//! - Messaging utilities with retry/flush policies
//! - OpenTelemetry trace context propagation
//! - Mock NATS clients for testing (with `test-support` feature)
//!
//! ## Example
//!
//! ```rust,no_run
//! use trogon_nats::{NatsConfig, connect};
//! use trogonstd::env::SystemEnv;
//!
//! #[tokio::main]
//! async fn main() {
//!     let config = NatsConfig::from_env(&SystemEnv);
//!     let client = connect(&config).await.expect("Failed to connect");
//! }
//! ```
//!
//! ## Zero-Cost Abstraction
//!
//! Use generics for zero-cost abstraction:
//!
//! ```rust,no_run
//! use trogon_nats::{RequestClient, PublishClient};
//!
//! // Depend only on the operations you need
//! pub struct MyService<N: RequestClient + PublishClient> {
//!     nats: N,
//! }
//! ```

pub mod auth;
pub mod client;
pub mod connect;
pub mod messaging;

#[cfg(feature = "test-support")]
pub mod mocks;

pub use auth::{NatsAuth, NatsConfig};
pub use client::{FlushClient, PublishClient, RequestClient, SubscribeClient};
pub use connect::{ConnectError, connect};
pub use messaging::{
    FlushPolicy, NatsError, PublishOperationError, PublishOptions, PublishOptionsBuilder,
    RetryPolicy, headers_with_trace_context, inject_trace_context, publish, request,
    request_with_timeout,
};

#[cfg(feature = "test-support")]
pub use mocks::{AdvancedMockNatsClient, MockNatsClient};

#![cfg_attr(test, allow(clippy::expect_used, clippy::panic, clippy::unwrap_used))]

//! OTLP tracing and Prometheus scrape helpers shared by sandbox binaries.

mod bootstrap;
mod metrics_addr;
mod metrics_server;
mod observability_args;
mod observability_error;
mod ports;
mod registry;
mod tracing_guard;
mod tracing_init;

pub use bootstrap::bootstrap;
pub use metrics_addr::metrics_addr;
pub use metrics_server::{serve_metrics, spawn_metrics_server};
pub use observability_args::ObservabilityArgs;
pub use observability_error::ObservabilityError;
pub use ports::*;
pub use registry::{default_registry, gather_text};
pub use tracing_guard::TracingGuard;
pub use tracing_init::{OTLP_ENDPOINT_ENV, init_tracing, init_tracing_with_env};

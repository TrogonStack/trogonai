#![cfg_attr(test, allow(clippy::expect_used, clippy::panic, clippy::unwrap_used))]

//! OTLP tracing, operational health paths and Prometheus scrape helpers shared by sandbox binaries.

mod bootstrap;
mod constants;
mod health;
mod metrics_addr;
mod metrics_server;
mod observability_args;
mod observability_error;
mod registry;
mod tracing_guard;
mod tracing_init;

pub use bootstrap::bootstrap;
pub use constants::{
    ACA_WORKER, BROKER, CREDENTIAL_BROKER, DASHBOARD, DAYTONA_WORKER, DEPENDENCY_GRACE, DOCKER_WORKER, E2B_WORKER,
    FIRECRACKER_WORKER, HYPERLIGHT_WORKER, K8S_WORKER, KUBEVIRT_WORKER, MODAL_WORKER, OTLP_ENDPOINT_ENV,
    SNAPSHOT_BUILDER, SPACE_WORKER, SSH_GATEWAY, SUBPROCESS_WORKER, SUBSTRATE_WORKER, V8_WASM_WORKER, VERCEL_WORKER,
    WS_GATEWAY,
};
pub use health::{Dependency, DependencyState, Health, Liveness, NatsDependency, NotReady, Readiness};
pub use metrics_addr::metrics_addr;
pub use metrics_server::{serve_metrics, serve_metrics_on, spawn_metrics_server};
pub use observability_args::ObservabilityArgs;
pub use observability_error::ObservabilityError;
pub use registry::{default_registry, gather_text};
pub use tracing_guard::TracingGuard;
pub use tracing_init::{init_tracing, init_tracing_with_env};

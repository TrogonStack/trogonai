use std::net::SocketAddr;

use clap::Parser;

use crate::{Health, metrics_addr};

/// CLI flags for OTLP export and the private listener serving health paths and Prometheus metrics.
#[derive(Clone, Debug, Parser)]
pub struct ObservabilityArgs {
    /// Full OTLP/HTTP traces URL, for example `http://collector:4318/v1/traces`. No-op when unset.
    #[arg(long, env = "SANDBOX_OTLP_ENDPOINT")]
    pub otlp_endpoint: Option<String>,

    /// Prometheus scrape address. Defaults to `127.0.0.1:<service_port + 1000>`.
    #[arg(long, env = "SANDBOX_METRICS_ADDR")]
    pub metrics_addr: Option<SocketAddr>,

    /// Health of this process. Every clone shares one state, so code that only holds the
    /// parsed arguments can mark the process started or require a dependency.
    #[arg(skip)]
    health: Health,
}

impl ObservabilityArgs {
    /// The health state served on `/-/liveness` and `/-/readiness`.
    pub fn health(&self) -> &Health {
        &self.health
    }

    /// The configured scrape address, or the default for `service_port`.
    pub fn resolve_metrics_addr(&self, service_port: u16) -> SocketAddr {
        self.metrics_addr.unwrap_or_else(|| metrics_addr(service_port))
    }
}

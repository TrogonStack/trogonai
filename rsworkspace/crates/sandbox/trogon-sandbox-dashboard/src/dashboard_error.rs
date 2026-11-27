/// Failures while running the dashboard server.
#[derive(Debug, thiserror::Error)]
pub enum DashboardError {
    #[error("failed to bind the dashboard listener: {0}")]
    Bind(#[source] std::io::Error),
    #[error("dashboard server failed: {0}")]
    Serve(#[source] std::io::Error),
    #[error("failed to install the shutdown signal handler: {0}")]
    Signal(#[source] std::io::Error),
    #[error("failed to set up observability: {0}")]
    Observability(#[from] trogon_sandbox_observability::ObservabilityError),
}

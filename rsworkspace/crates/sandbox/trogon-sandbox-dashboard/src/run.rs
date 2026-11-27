use std::future::Future;

use tokio::net::TcpListener;
use trogon_sandbox_observability::DASHBOARD;

use crate::{DashboardArgs, DashboardConfig, DashboardError, router, serve_app_with_shutdown};

/// Starts observability and serves the dashboard until `shutdown` resolves.
pub async fn run<F>(args: DashboardArgs, shutdown: F) -> Result<(), DashboardError>
where
    F: Future<Output = Result<(), DashboardError>> + Send + 'static,
{
    let (_tracing, _registry) =
        trogon_sandbox_observability::bootstrap("trogon-sandbox-dashboard", DASHBOARD, &args.observability)?;
    let health = args.observability.health().clone();
    let listener = TcpListener::bind(args.listen).await.map_err(DashboardError::Bind)?;
    tracing::info!(listen = %args.listen, "sandbox dashboard listening");
    let config = DashboardConfig {
        ws_gateway_url: args.ws_gateway_url,
    };
    let app = router(config).merge(health.router());
    health.mark_started();
    let stopping = health.clone();
    serve_app_with_shutdown(listener, app, async move {
        let outcome = shutdown.await;
        stopping.begin_shutdown();
        outcome
    })
    .await
}

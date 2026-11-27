use std::future::Future;

use axum::Router;
use tokio::net::TcpListener;

use crate::{DashboardConfig, DashboardError, router};

/// Serves the dashboard on an already bound listener until `shutdown` resolves.
pub async fn serve_with_shutdown<F>(
    listener: TcpListener,
    config: DashboardConfig,
    shutdown: F,
) -> Result<(), DashboardError>
where
    F: Future<Output = Result<(), DashboardError>> + Send + 'static,
{
    serve_app_with_shutdown(listener, router(config), shutdown).await
}

/// Serves `app` on an already bound listener until `shutdown` resolves.
pub async fn serve_app_with_shutdown<F>(listener: TcpListener, app: Router, shutdown: F) -> Result<(), DashboardError>
where
    F: Future<Output = Result<(), DashboardError>> + Send + 'static,
{
    let (signal_error, mut signal_failed) = tokio::sync::oneshot::channel();
    let graceful = async move {
        if let Err(err) = shutdown.await {
            let _ = signal_error.send(err);
        }
    };
    axum::serve(listener, app)
        .with_graceful_shutdown(graceful)
        .await
        .map_err(DashboardError::Serve)?;
    match signal_failed.try_recv() {
        Ok(err) => Err(err),
        Err(_) => Ok(()),
    }
}

/// Resolves when Ctrl+C is received.
pub async fn shutdown_signal() -> Result<(), DashboardError> {
    tokio::signal::ctrl_c().await.map_err(DashboardError::Signal)
}

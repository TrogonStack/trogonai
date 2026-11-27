use std::future::Future;

use axum::Router;
use tokio::net::TcpListener;

use crate::GatewayError;

/// Serves `app` on an already bound listener until `shutdown` resolves.
pub async fn serve_with_shutdown<F>(listener: TcpListener, app: Router, shutdown: F) -> Result<(), GatewayError>
where
    F: Future<Output = Result<(), GatewayError>> + Send + 'static,
{
    let (signal_error, mut signal_failed) = tokio::sync::oneshot::channel();
    let graceful = async move {
        if let Err(error) = shutdown.await {
            let _ = signal_error.send(error);
        }
    };
    axum::serve(listener, app)
        .with_graceful_shutdown(graceful)
        .await
        .map_err(GatewayError::Serve)?;
    match signal_failed.try_recv() {
        Ok(error) => Err(error),
        Err(_) => Ok(()),
    }
}

/// Resolves when Ctrl+C is received.
pub async fn shutdown_signal() -> Result<(), GatewayError> {
    tokio::signal::ctrl_c().await.map_err(GatewayError::Signal)
}

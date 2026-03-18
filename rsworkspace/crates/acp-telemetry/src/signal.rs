use tracing::info;
use tracing::warn;

pub async fn shutdown_signal() {
    let ctrl_c = async {
        if let Err(error) = tokio::signal::ctrl_c().await {
            warn!(error = %error, "Failed to install Ctrl+C handler");
            std::future::pending::<()>().await;
            return;
        }
        info!("Received SIGINT (Ctrl+C)");
    };

    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut signal) => {
                signal.recv().await;
                info!("Received SIGTERM");
            }
            Err(error) => {
                warn!(error = %error, "Failed to install SIGTERM handler");
                std::future::pending::<()>().await;
            }
        }
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {}
        _ = terminate => {}
    }
}

use tracing::error;

pub fn init_logging() {
    let env_filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(env_filter)
        .with_writer(std::io::stderr)
        .try_init();
}

pub async fn shutdown_signal() {
    #[cfg(unix)]
    {
        let mut terminate = match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(signal) => Some(signal),
            Err(error) => {
                error!(error = %error, "Failed to listen for SIGTERM");
                None
            }
        };

        tokio::select! {
            result = tokio::signal::ctrl_c() => {
                if let Err(error) = result {
                    error!(error = %error, "Failed to listen for shutdown signal");
                }
            }
            _ = async {
                if let Some(signal) = terminate.as_mut() {
                    signal.recv().await;
                } else {
                    std::future::pending::<()>().await;
                }
            } => {}
        }
    }

    #[cfg(not(unix))]
    {
        if let Err(error) = tokio::signal::ctrl_c().await {
            error!(error = %error, "Failed to listen for shutdown signal");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::shutdown_signal;
    use tokio::time::{Duration, timeout};

    #[tokio::test]
    async fn shutdown_signal_waits_for_shutdown_event() {
        let result = timeout(Duration::from_millis(10), shutdown_signal()).await;

        assert!(result.is_err());
    }
}

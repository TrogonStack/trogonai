use crate::auth::{NatsAuth, NatsConfig};
use async_nats::{Client, ConnectOptions, Event};
use std::time::Duration;
use tracing::{info, instrument, warn};

#[derive(Debug)]
pub enum ConnectError {
    InvalidCredentials(std::io::Error),
    ConnectionFailed {
        servers: Vec<String>,
        error: async_nats::ConnectError,
    },
}

impl std::fmt::Display for ConnectError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidCredentials(e) => {
                write!(f, "Failed to load credentials file: {}", e)
            }
            Self::ConnectionFailed { servers, error } => {
                write!(
                    f,
                    "Failed to connect to NATS servers {:?}: {}",
                    servers, error
                )
            }
        }
    }
}

impl std::error::Error for ConnectError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::InvalidCredentials(e) => Some(e),
            Self::ConnectionFailed { error, .. } => Some(error),
        }
    }
}

const CONNECTION_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_RECONNECT_DELAY: Duration = Duration::from_secs(30);

fn reconnect_delay(attempts: usize) -> Duration {
    let delay = Duration::from_secs(std::cmp::min(
        MAX_RECONNECT_DELAY.as_secs(),
        2u64.saturating_pow(attempts as u32),
    ));
    info!(
        attempts,
        delay_secs = delay.as_secs(),
        "NATS reconnect delay"
    );
    delay
}

async fn handle_event(event: Event) {
    match event {
        Event::Connected => info!("NATS connected"),
        Event::Disconnected => warn!("NATS disconnected - will attempt reconnect"),
        Event::ServerError(err) => warn!(error = %err, "NATS server error"),
        Event::ClientError(err) => warn!(error = %err, "NATS client error"),
        Event::SlowConsumer(sid) => warn!(sid, "NATS slow consumer detected"),
        Event::LameDuckMode => warn!("NATS server entering lame duck mode"),
        Event::Closed => info!("NATS connection closed"),
        Event::Draining => info!("NATS connection draining"),
    }
}

fn apply_reconnect_options(opts: ConnectOptions) -> ConnectOptions {
    opts.retry_on_initial_connect()
        .connection_timeout(CONNECTION_TIMEOUT)
        .reconnect_delay_callback(reconnect_delay)
        .event_callback(|event| async move { handle_event(event).await })
}

/// Connect to NATS with automatic reconnection and event handling.
#[instrument(name = "nats.connect", skip(config), fields(servers = ?config.servers, auth = %config.auth.description()))]
pub async fn connect(config: &NatsConfig) -> Result<Client, ConnectError> {
    info!(
        servers = ?config.servers,
        auth = %config.auth.description(),
        "Connecting to NATS"
    );

    let connect_result = match &config.auth {
        NatsAuth::Credentials(path) => {
            info!(path = %path.display(), "Using credentials file");
            match ConnectOptions::with_credentials_file(path.clone()).await {
                Ok(opts) => apply_reconnect_options(opts).connect(&config.servers).await,
                Err(e) => {
                    warn!(error = %e, path = %path.display(), "Failed to load credentials file");
                    return Err(ConnectError::InvalidCredentials(e));
                }
            }
        }
        NatsAuth::NKey(seed) => {
            apply_reconnect_options(ConnectOptions::with_nkey(seed.clone()))
                .connect(&config.servers)
                .await
        }
        NatsAuth::UserPassword { user, password } => {
            apply_reconnect_options(ConnectOptions::with_user_and_password(
                user.clone(),
                password.clone(),
            ))
            .connect(&config.servers)
            .await
        }
        NatsAuth::Token(token) => {
            apply_reconnect_options(ConnectOptions::with_token(token.clone()))
                .connect(&config.servers)
                .await
        }
        NatsAuth::None => {
            apply_reconnect_options(ConnectOptions::new())
                .connect(&config.servers)
                .await
        }
    };

    match connect_result {
        Ok(client) => {
            info!(
                servers = ?config.servers,
                auth = %config.auth.description(),
                "Connected to NATS"
            );
            Ok(client)
        }
        Err(e) => {
            warn!(
                error = %e,
                servers = ?config.servers,
                auth = %config.auth.description(),
                "Failed to connect to NATS"
            );
            Err(ConnectError::ConnectionFailed {
                servers: config.servers.clone(),
                error: e,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_reconnect_delay_starts_at_one_second() {
        assert_eq!(reconnect_delay(0).as_secs(), 1);
    }

    #[test]
    fn test_reconnect_delay_exponential_backoff() {
        assert_eq!(reconnect_delay(0).as_secs(), 1); // 2^0 = 1
        assert_eq!(reconnect_delay(1).as_secs(), 2); // 2^1 = 2
        assert_eq!(reconnect_delay(2).as_secs(), 4); // 2^2 = 4
        assert_eq!(reconnect_delay(3).as_secs(), 8); // 2^3 = 8
        assert_eq!(reconnect_delay(4).as_secs(), 16); // 2^4 = 16
    }

    #[test]
    fn test_reconnect_delay_caps_at_max() {
        assert_eq!(reconnect_delay(5).as_secs(), 30);
        assert_eq!(reconnect_delay(10).as_secs(), 30);
        assert_eq!(reconnect_delay(100).as_secs(), 30);
    }

    #[test]
    fn test_reconnect_delay_overflow_protection() {
        assert_eq!(reconnect_delay(usize::MAX).as_secs(), 30);
    }

    #[tokio::test]
    async fn test_handle_event_connected() {
        handle_event(Event::Connected).await;
    }

    #[tokio::test]
    async fn test_handle_event_disconnected() {
        handle_event(Event::Disconnected).await;
    }

    #[tokio::test]
    async fn test_handle_event_all_variants() {
        use async_nats::{ClientError, ServerError};

        handle_event(Event::Connected).await;
        handle_event(Event::Disconnected).await;
        handle_event(Event::ServerError(ServerError::Other("test".to_string()))).await;
        handle_event(Event::ClientError(ClientError::Other("test".to_string()))).await;
        handle_event(Event::SlowConsumer(42)).await;
        handle_event(Event::LameDuckMode).await;
        handle_event(Event::Closed).await;
        handle_event(Event::Draining).await;
    }

    #[test]
    fn test_connection_timeout_constant() {
        assert_eq!(CONNECTION_TIMEOUT.as_secs(), 10);
    }

    #[test]
    fn test_max_reconnect_delay_constant() {
        assert_eq!(MAX_RECONNECT_DELAY.as_secs(), 30);
    }

    #[test]
    fn connect_error_display_invalid_credentials() {
        let err = ConnectError::InvalidCredentials(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "file not found",
        ));
        let msg = err.to_string();
        assert!(msg.contains("Failed to load credentials file"));
        assert!(msg.contains("file not found"));
    }

    #[test]
    fn connect_error_source_invalid_credentials() {
        let err = ConnectError::InvalidCredentials(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "file not found",
        ));
        assert!(std::error::Error::source(&err).is_some());
    }
}

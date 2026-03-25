use crate::auth::{NatsAuth, NatsConfig};
use async_nats::{Client, ClientError, ConnectOptions, Event};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::oneshot;
use tracing::{info, instrument, warn};

#[derive(Debug)]
pub enum ConnectError {
    InvalidCredentials(std::io::Error),
    /// NATS server rejected the connection due to invalid credentials.
    /// Retrying will not help — the credentials must be corrected.
    AuthorizationViolation,
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
            Self::AuthorizationViolation => {
                write!(f, "NATS authorization violation: invalid credentials")
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
            Self::AuthorizationViolation => None,
            Self::ConnectionFailed { error, .. } => Some(error),
        }
    }
}

const MAX_RECONNECT_DELAY: Duration = Duration::from_secs(30);

/// How long to wait for the initial connection outcome before assuming the server
/// is temporarily unreachable and letting the retry loop continue in the background.
const INITIAL_CONNECT_CHECK_SECS: u64 = 3;


fn reconnect_delay(attempts: usize) -> Duration {
    // Attempt 1 is the initial connection — connect immediately (no delay).
    // Subsequent attempts use exponential backoff up to MAX_RECONNECT_DELAY.
    if attempts <= 1 {
        return Duration::ZERO;
    }
    let delay = Duration::from_secs(std::cmp::min(
        MAX_RECONNECT_DELAY.as_secs(),
        2u64.saturating_pow((attempts - 1) as u32),
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

/// `outcome_tx` is a one-shot used only during startup:
///   - `true`  → `Event::Connected` (auth ok)
///   - `false` → `Event::ClientError` with "authorization violation"
fn apply_reconnect_options(
    opts: ConnectOptions,
    connection_timeout: Duration,
    outcome_tx: Arc<Mutex<Option<oneshot::Sender<bool>>>>,
) -> ConnectOptions {
    opts.retry_on_initial_connect()
        .connection_timeout(connection_timeout)
        .reconnect_delay_callback(reconnect_delay)
        .event_callback(move |event| {
            let tx = outcome_tx.clone();
            async move {
                let signal: Option<bool> = match &event {
                    Event::Connected => Some(true),
                    Event::ClientError(ClientError::Other(msg))
                        if msg.contains("authorization violation") =>
                    {
                        Some(false)
                    }
                    _ => None,
                };
                if let Some(ok) = signal
                    && let Ok(mut guard) = tx.lock()
                    && let Some(sender) = guard.take()
                {
                    let _ = sender.send(ok);
                }
                handle_event(event).await;
            }
        })
}

#[instrument(name = "nats.connect", skip(config), fields(servers = ?config.servers, auth = %config.auth.description(), timeout_secs = ?connection_timeout.as_secs()))]
pub async fn connect(
    config: &NatsConfig,
    connection_timeout: Duration,
) -> Result<Client, ConnectError> {
    info!(
        servers = ?config.servers,
        auth = %config.auth.description(),
        "Connecting to NATS"
    );

    // One-shot used to detect the first meaningful outcome of the initial
    // connection attempt: true = connected, false = authorization violation.
    // With `retry_on_initial_connect()` the async_nats `connect()` call
    // returns a Client immediately and the handshake happens in a background
    // task, so we need this side-channel to observe the result.
    let (outcome_tx, outcome_rx) = oneshot::channel::<bool>();
    let outcome_tx = Arc::new(Mutex::new(Some(outcome_tx)));

    let connect_result = match &config.auth {
        NatsAuth::Credentials(path) => {
            info!(path = %path.display(), "Using credentials file");
            match ConnectOptions::with_credentials_file(path.clone()).await {
                Ok(opts) => {
                    apply_reconnect_options(opts, connection_timeout, outcome_tx)
                        .connect(&config.servers)
                        .await
                }
                Err(e) => {
                    warn!(error = %e, path = %path.display(), "Failed to load credentials file");
                    return Err(ConnectError::InvalidCredentials(e));
                }
            }
        }
        NatsAuth::NKey(seed) => {
            apply_reconnect_options(
                ConnectOptions::with_nkey(seed.clone()),
                connection_timeout,
                outcome_tx,
            )
            .connect(&config.servers)
            .await
        }
        NatsAuth::UserPassword { user, password } => {
            apply_reconnect_options(
                ConnectOptions::with_user_and_password(user.clone(), password.clone()),
                connection_timeout,
                outcome_tx,
            )
            .connect(&config.servers)
            .await
        }
        NatsAuth::Token(token) => {
            apply_reconnect_options(
                ConnectOptions::with_token(token.clone()),
                connection_timeout,
                outcome_tx,
            )
            .connect(&config.servers)
            .await
        }
        NatsAuth::None => {
            apply_reconnect_options(ConnectOptions::new(), connection_timeout, outcome_tx)
                .connect(&config.servers)
                .await
        }
    };

    let client = match connect_result {
        Ok(client) => client,
        Err(e) => {
            warn!(
                error = %e,
                servers = ?config.servers,
                auth = %config.auth.description(),
                "Failed to connect to NATS"
            );
            return Err(ConnectError::ConnectionFailed {
                servers: config.servers.clone(),
                error: e,
            });
        }
    };

    // Wait for the background handshake to report an outcome.
    // - If the server is reachable and accepts the credentials → Connected event fires quickly.
    // - If the server rejects the credentials → auth violation event fires quickly → fail fast.
    // - If the server is unreachable → no event fires within the check window → return the
    //   client and let the retry loop continue in the background (desired resilience behaviour).
    //
    // We use INITIAL_CONNECT_CHECK_SECS (not the full connection_timeout) so that a temporarily
    // unavailable server does not stall startup for the full per-connection timeout.
    let check_window = Duration::from_secs(INITIAL_CONNECT_CHECK_SECS);
    tokio::select! {
        outcome = outcome_rx => {
            match outcome {
                Ok(false) => {
                    warn!(
                        servers = ?config.servers,
                        auth = %config.auth.description(),
                        "NATS authorization violation — check credentials"
                    );
                    return Err(ConnectError::AuthorizationViolation);
                }
                Ok(true) => {
                    info!(
                        servers = ?config.servers,
                        auth = %config.auth.description(),
                        "Connected to NATS"
                    );
                }
                Err(_) => {
                    // Sender dropped without sending (should not happen in practice).
                }
            }
        }
        _ = tokio::time::sleep(check_window) => {
            // Server is not reachable yet; retry continues in the background.
            info!(
                servers = ?config.servers,
                auth = %config.auth.description(),
                "NATS server not yet reachable, retrying in background"
            );
        }
    }

    Ok(client)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_reconnect_delay_first_attempt_is_immediate() {
        // Attempt 1 is the initial connect — no delay.
        assert_eq!(reconnect_delay(0).as_millis(), 0);
        assert_eq!(reconnect_delay(1).as_millis(), 0);
    }

    #[test]
    fn test_reconnect_delay_exponential_backoff() {
        // Attempts 2+ use exponential backoff: 2^(attempt-1) seconds.
        assert_eq!(reconnect_delay(2).as_secs(), 2);
        assert_eq!(reconnect_delay(3).as_secs(), 4);
        assert_eq!(reconnect_delay(4).as_secs(), 8);
        assert_eq!(reconnect_delay(5).as_secs(), 16);
    }

    #[test]
    fn test_reconnect_delay_caps_at_max() {
        assert_eq!(reconnect_delay(6).as_secs(), 30);
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

    #[tokio::test]
    async fn connect_with_missing_credentials_file_returns_invalid_credentials() {
        let config = NatsConfig::new(
            vec!["nats://127.0.0.1:4222".to_string()],
            NatsAuth::Credentials("/nonexistent/path/to/creds.nk".into()),
        );

        let err = connect(&config, Duration::from_millis(100))
            .await
            .expect_err("expected an error for missing credentials file");

        assert!(matches!(err, ConnectError::InvalidCredentials(_)));
    }
}

use crate::auth::{NatsAuth, NatsConfig};
use crate::constants::MAX_RECONNECT_DELAY;
use async_nats::{Client, ClientError, ConnectOptions, Event, ServerError};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::oneshot;
use tracing::{info, instrument, warn};
use trogon_semconv::span::NATS_CONNECT;

#[derive(Debug, thiserror::Error)]
pub enum ConnectError {
    #[error("Failed to load credentials file: {0}")]
    InvalidCredentials(#[source] std::io::Error),
    /// NATS server rejected the connection due to invalid credentials.
    /// Retrying will not help — the credentials must be corrected.
    #[error("NATS authorization violation: invalid credentials")]
    AuthorizationViolation,
    #[error("Failed to connect to NATS servers {servers:?}: {error}")]
    ConnectionFailed {
        servers: Vec<String>,
        #[source]
        error: async_nats::ConnectError,
    },
}

/// How long to wait for the initial connection outcome before assuming the server
/// is temporarily unreachable and letting the retry loop continue in the background.
const INITIAL_CONNECT_CHECK_SECS: u64 = 3;

#[cfg_attr(coverage, coverage(off))]
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
    info!(attempts, delay_secs = delay.as_secs(), "NATS reconnect delay");
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
#[cfg_attr(coverage, coverage(off))]
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
                    Event::ServerError(ServerError::AuthorizationViolation) => Some(false),
                    Event::ClientError(ClientError::Other(msg)) if msg.contains("authorization violation") => {
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

#[cfg_attr(coverage, coverage(off))]
#[instrument(name = NATS_CONNECT, skip(config), fields(servers = ?config.servers, auth = %config.auth.description(), timeout_secs = ?connection_timeout.as_secs()))]
pub async fn connect(config: &NatsConfig, connection_timeout: Duration) -> Result<Client, ConnectError> {
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
            apply_reconnect_options(ConnectOptions::with_nkey(seed.clone()), connection_timeout, outcome_tx)
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
mod tests;

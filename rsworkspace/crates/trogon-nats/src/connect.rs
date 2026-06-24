use crate::auth::{NatsAuth, NatsConfig};
use crate::constants::MAX_RECONNECT_DELAY;
use async_nats::{Client, ConnectOptions, Event};
use std::time::Duration;
use tracing::{info, instrument, warn};

#[derive(Debug, thiserror::Error)]
pub enum ConnectError {
    #[error("Failed to load credentials file: {0}")]
    InvalidCredentials(#[source] std::io::Error),
    #[error("Failed to connect to NATS servers {servers:?}: {error}")]
    ConnectionFailed {
        servers: Vec<String>,
        #[source]
        error: async_nats::ConnectError,
    },
}

fn reconnect_delay(attempts: usize) -> Duration {
    let delay = Duration::from_secs(std::cmp::min(
        MAX_RECONNECT_DELAY.as_secs(),
        2u64.saturating_pow(attempts as u32),
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

fn apply_reconnect_options(opts: ConnectOptions, connection_timeout: Duration) -> ConnectOptions {
    opts.retry_on_initial_connect()
        .connection_timeout(connection_timeout)
        .reconnect_delay_callback(reconnect_delay)
        .event_callback(|event| async move { handle_event(event).await })
}

#[instrument(name = "nats.connect", skip(config), fields(servers = ?config.servers, auth = %config.auth.description(), timeout_secs = ?connection_timeout.as_secs()))]
pub async fn connect(config: &NatsConfig, connection_timeout: Duration) -> Result<Client, ConnectError> {
    info!(
        servers = ?config.servers,
        auth = %config.auth.description(),
        "Connecting to NATS"
    );

    let connect_result = match &config.auth {
        NatsAuth::Credentials(path) => {
            info!(path = %path.display(), "Using credentials file");
            match ConnectOptions::with_credentials_file(path.clone()).await {
                Ok(opts) => {
                    apply_reconnect_options(opts, connection_timeout)
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
            apply_reconnect_options(ConnectOptions::with_nkey(seed.clone()), connection_timeout)
                .connect(&config.servers)
                .await
        }
        NatsAuth::UserPassword { user, password } => {
            apply_reconnect_options(
                ConnectOptions::with_user_and_password(user.clone(), password.clone()),
                connection_timeout,
            )
            .connect(&config.servers)
            .await
        }
        NatsAuth::Token(token) => {
            apply_reconnect_options(ConnectOptions::with_token(token.clone()), connection_timeout)
                .connect(&config.servers)
                .await
        }
        NatsAuth::None => {
            apply_reconnect_options(ConnectOptions::new(), connection_timeout)
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
mod tests;

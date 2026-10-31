use crate::auth::{NatsAuth, NatsConfig};
use crate::constants::{
    ENV_NATS_CA, ENV_NATS_CERT, ENV_NATS_CREDS, ENV_NATS_JWT, ENV_NATS_KEY, ENV_NATS_NKEY, ENV_NATS_PASSWORD,
    ENV_NATS_SEED, ENV_NATS_TOKEN, MAX_RECONNECT_DELAY,
};
use crate::credential_value::{CredentialValue, SettingError};
use crate::tls::NatsTls;
use async_nats::{AuthError, Client, ConnectOptions, Event};
use nkeys::KeyPair;
use std::sync::Arc;
use std::time::Duration;
use tracing::{info, instrument, warn};
use trogon_semconv::span::NATS_CONNECT;
use trogon_std::env::{ReadEnv, SystemEnv};

#[derive(Debug, thiserror::Error)]
pub enum ConnectError {
    #[error("Failed to load credentials file: {0}")]
    InvalidCredentials(#[source] std::io::Error),
    #[error("Invalid {setting}: {error}")]
    InvalidSetting {
        setting: &'static str,
        #[source]
        error: SettingError,
    },
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

async fn apply_auth<E: ReadEnv>(
    opts: ConnectOptions,
    auth: &NatsAuth,
    env: &E,
) -> Result<ConnectOptions, ConnectError> {
    match auth {
        NatsAuth::Credentials(location) => {
            let path = location.path().map_err(invalid_setting(ENV_NATS_CREDS))?;
            info!(path = %path.display(), "Using credentials file");
            opts.credentials_file(path).await.map_err(|e| {
                warn!(error = %e, path = %path.display(), "Failed to load credentials file");
                ConnectError::InvalidCredentials(e)
            })
        }
        NatsAuth::NKey(seed) => {
            let seed = seed.resolve(env).await.map_err(invalid_setting(ENV_NATS_NKEY))?;
            Ok(opts.nkey(seed))
        }
        NatsAuth::UserPassword { user, password } => {
            let password = password
                .resolve(env)
                .await
                .map_err(invalid_setting(ENV_NATS_PASSWORD))?;
            Ok(opts.user_and_password(user.clone(), password))
        }
        NatsAuth::Jwt { jwt, seed } => {
            let jwt = resolve_jwt(jwt, env).await?;
            match seed {
                Some(seed) => {
                    let seed = seed.resolve(env).await.map_err(invalid_setting(ENV_NATS_SEED))?;
                    let key_pair = Arc::new(
                        KeyPair::from_seed(seed.trim())
                            .map_err(|e| invalid_setting(ENV_NATS_SEED)(SettingError::InvalidSeed(e)))?,
                    );
                    Ok(opts.jwt(jwt, jwt_signer(key_pair)))
                }
                None => Ok(opts.jwt(jwt, |_| std::future::ready(Ok(Vec::new())))),
            }
        }
        NatsAuth::Token(token) => {
            let token = token.resolve(env).await.map_err(invalid_setting(ENV_NATS_TOKEN))?;
            Ok(opts.token(token))
        }
        NatsAuth::None => Ok(opts),
    }
}

async fn resolve_jwt<E: ReadEnv>(jwt: &CredentialValue, env: &E) -> Result<String, ConnectError> {
    let jwt = jwt.resolve(env).await.map_err(invalid_setting(ENV_NATS_JWT))?;
    Ok(jwt.trim().to_owned())
}

fn jwt_signer(
    key_pair: Arc<KeyPair>,
) -> impl Fn(Vec<u8>) -> std::future::Ready<Result<Vec<u8>, AuthError>> + Send + Sync + 'static {
    move |nonce| std::future::ready(key_pair.sign(&nonce).map_err(AuthError::new))
}

fn apply_tls(opts: ConnectOptions, tls: &NatsTls) -> Result<ConnectOptions, ConnectError> {
    if !tls.is_configured() {
        return Ok(opts);
    }
    let mut opts = opts.require_tls(true);
    let client_certificate = tls
        .client_certificate()
        .map_err(|incomplete| ConnectError::InvalidSetting {
            setting: incomplete.present_setting(),
            error: SettingError::MissingCompanion(incomplete.missing_setting()),
        })?;
    if let Some(client) = client_certificate {
        let cert = client.cert.path().map_err(invalid_setting(ENV_NATS_CERT))?;
        let key = client.key.path().map_err(invalid_setting(ENV_NATS_KEY))?;
        opts = opts.add_client_certificate(cert.to_path_buf(), key.to_path_buf());
    }
    if let Some(ca) = tls.root_ca() {
        let ca = ca.path().map_err(invalid_setting(ENV_NATS_CA))?;
        opts = opts.add_root_certificates(ca.to_path_buf());
    }
    Ok(opts)
}

async fn build_options<E: ReadEnv>(config: &NatsConfig, env: &E) -> Result<ConnectOptions, ConnectError> {
    let opts = apply_auth(ConnectOptions::new(), &config.auth, env).await?;
    apply_tls(opts, &config.tls)
}

fn invalid_setting(setting: &'static str) -> impl Fn(SettingError) -> ConnectError {
    move |error| ConnectError::InvalidSetting { setting, error }
}

#[instrument(name = NATS_CONNECT, skip(config), fields(servers = ?config.servers, auth = %config.auth.description(), timeout_secs = ?connection_timeout.as_secs()))]
pub async fn connect(config: &NatsConfig, connection_timeout: Duration) -> Result<Client, ConnectError> {
    info!(
        servers = ?config.servers,
        auth = %config.auth.description(),
        tls = config.tls.is_configured(),
        "Connecting to NATS"
    );

    let opts = build_options(config, &SystemEnv).await?;
    let connect_result = apply_reconnect_options(opts, connection_timeout)
        .connect(&config.servers)
        .await;

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

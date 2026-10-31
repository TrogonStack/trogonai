use crate::auth::{NatsAuth, NatsConfig};
use crate::constants::{
    ENV_NATS_CA, ENV_NATS_CERT, ENV_NATS_CREDS, ENV_NATS_JWT, ENV_NATS_KEY, ENV_NATS_NKEY, ENV_NATS_PASSWORD,
    ENV_NATS_SEED, ENV_NATS_TLS_DISABLED, ENV_NATS_TOKEN, FIRST_CONNECT_ATTEMPT, MAX_RECONNECT_DELAY,
    TLS_DISABLED_VALUES,
};
use crate::credential_value::{CredentialValue, SettingError};
use crate::tls::{NatsTls, TlsMisconfiguration};
use async_nats::{AuthError, Client, ConnectOptions, Event};
use nkeys::KeyPair;
use std::sync::Arc;
use std::time::Duration;
use tracing::{info, instrument, warn};
use trogon_semconv::span::NATS_CONNECT;
use trogon_std::env::{ReadEnv, SystemEnv};
use trogon_std::tls::install_default_crypto_provider;

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
    #[error("No NATS servers to connect to")]
    NoServers,
}

fn reconnect_delay(attempts: usize) -> Duration {
    if attempts <= FIRST_CONNECT_ATTEMPT {
        return Duration::ZERO;
    }
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
    let NatsTls::Enabled { ca, client_cert } = tls else {
        return Ok(opts);
    };
    let mut opts = opts.require_tls(true);
    if let Some(client) = client_cert {
        let cert = client.cert.path().map_err(invalid_setting(ENV_NATS_CERT))?;
        let key = client.key.path().map_err(invalid_setting(ENV_NATS_KEY))?;
        opts = opts.add_client_certificate(cert.to_path_buf(), key.to_path_buf());
    }
    if let Some(ca) = ca {
        let ca = ca.path().map_err(invalid_setting(ENV_NATS_CA))?;
        opts = opts.add_root_certificates(ca.to_path_buf());
    }
    Ok(opts)
}

fn tls(config: &NatsConfig) -> Result<&NatsTls, ConnectError> {
    config.tls.as_ref().map_err(invalid_tls_setting)
}

pub(crate) async fn build_options<E: ReadEnv>(config: &NatsConfig, env: &E) -> Result<ConnectOptions, ConnectError> {
    let tls = tls(config)?;
    let opts = apply_auth(ConnectOptions::new(), &config.auth, env).await?;
    apply_tls(opts, tls)
}

fn invalid_setting(setting: &'static str) -> impl Fn(SettingError) -> ConnectError {
    move |error| ConnectError::InvalidSetting { setting, error }
}

fn invalid_tls_setting(misconfiguration: &TlsMisconfiguration) -> ConnectError {
    match misconfiguration {
        TlsMisconfiguration::IncompleteClientCertificate(incomplete) => ConnectError::InvalidSetting {
            setting: incomplete.present_setting(),
            error: SettingError::MissingCompanion(incomplete.missing_setting()),
        },
        TlsMisconfiguration::UnrecognizedDisabledValue(value) => ConnectError::InvalidSetting {
            setting: ENV_NATS_TLS_DISABLED,
            error: SettingError::UnrecognizedValue {
                value: value.clone(),
                accepted: &TLS_DISABLED_VALUES,
            },
        },
        TlsMisconfiguration::DisabledAlongside(setting) => ConnectError::InvalidSetting {
            setting: ENV_NATS_TLS_DISABLED,
            error: SettingError::ConflictsWith(setting),
        },
    }
}

async fn prepare(config: &NatsConfig) -> Result<ConnectOptions, ConnectError> {
    install_default_crypto_provider();
    if config.servers.is_empty() {
        return Err(ConnectError::NoServers);
    }
    let tls = tls(config)?;
    info!(
        servers = ?config.servers,
        auth = %config.auth.description(),
        tls = %tls.description(),
        "Connecting to NATS"
    );
    if *tls == NatsTls::Disabled {
        warn!(
            servers = ?config.servers,
            "NATS TLS is disabled, which is only for tests and local debugging"
        );
    }

    build_options(config, &SystemEnv).await
}

fn connected(config: &NatsConfig, result: Result<Client, async_nats::ConnectError>) -> Result<Client, ConnectError> {
    match result {
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

/// Returns before any server answers and keeps dialing, then reconnecting, in
/// the background, so it only fails on settings it cannot use.
#[instrument(name = NATS_CONNECT, skip(config), fields(servers = ?config.servers, auth = %config.auth.description(), timeout_secs = ?connection_timeout.as_secs()))]
pub async fn connect(config: &NatsConfig, connection_timeout: Duration) -> Result<Client, ConnectError> {
    let opts = prepare(config).await?;
    let result = apply_reconnect_options(opts, connection_timeout)
        .connect(&config.servers)
        .await;
    connected(config, result)
}

/// Makes a single pass over the servers with `auth` in place of the configured
/// authentication and fails once that pass does, for a connection that serves
/// one request. TLS follows the same settings as [`connect`].
///
/// Give a caller's token as [`CredentialValue::Literal`] so it is never read as
/// a `file://` or `env://` reference.
#[instrument(name = NATS_CONNECT, skip(config, auth), fields(servers = ?config.servers, auth = %auth.description(), timeout_secs = ?connection_timeout.as_secs()))]
pub async fn connect_once(
    config: &NatsConfig,
    auth: NatsAuth,
    connection_timeout: Duration,
) -> Result<Client, ConnectError> {
    let config = NatsConfig { auth, ..config.clone() };
    let opts = prepare(&config).await?;
    let result = opts
        .connection_timeout(connection_timeout)
        .connect(&config.servers)
        .await;
    connected(&config, result)
}

#[cfg(test)]
mod tests;

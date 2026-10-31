use async_nats::connection::State;
use async_nats::{ClientError, ConnectErrorKind, ServerError};

use super::*;
use crate::credential_value::{CredentialValue, Location, NKeySeed};
use crate::server_info::wait_for_server_info;
use crate::test_support::CoreTestServer;
use crate::tls::ClientCertificate;
use tracing::subscriber::DefaultGuard;
use trogon_std::env::InMemoryEnv;
use trogon_std::log_capture::{CapturedEvents, LevelFilter};
use trogon_std::secret_string::SecretString;

const TLS_DISABLED_WARNING: &str = "NATS TLS is disabled, which is only for tests and local debugging";
const CONNECT_FAILURE_WARNING: &str = "Failed to connect to NATS";
const PLAINTEXT_CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const FIRST_DIAL_DEADLINE: Duration = Duration::from_secs(1);
const SERVER_INFO_POLL_INTERVAL: Duration = Duration::from_millis(10);
const REFUSED_DIAL_TIMEOUT: Duration = Duration::from_secs(1);
const FAIL_FAST_DEADLINE: Duration = Duration::from_secs(5);
const SHUFFLED_PASSES: usize = 32;
const REFUSING_SERVER: &str = "127.0.0.1:1";
const SERVER_TOKEN: &str = "server-token";
const OTHER_TOKEN: &str = "other-token";

#[test]
fn test_reconnect_delay_skips_the_wait_before_the_first_attempt() {
    assert_eq!(reconnect_delay(0), Duration::ZERO);
    assert_eq!(reconnect_delay(1), Duration::ZERO);
}

#[test]
fn test_reconnect_delay_exponential_backoff() {
    assert_eq!(reconnect_delay(2).as_secs(), 4);
    assert_eq!(reconnect_delay(3).as_secs(), 8);
    assert_eq!(reconnect_delay(4).as_secs(), 16);
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
    let err = ConnectError::InvalidCredentials(std::io::Error::new(std::io::ErrorKind::NotFound, "file not found"));
    assert_eq!(err.to_string(), "Failed to load credentials file: file not found");
}

#[test]
fn connect_error_source_invalid_credentials() {
    let err = ConnectError::InvalidCredentials(std::io::Error::new(std::io::ErrorKind::NotFound, "file not found"));
    assert!(std::error::Error::source(&err).is_some());
}

#[tokio::test]
async fn connect_with_missing_credentials_file_returns_invalid_credentials() {
    let config = NatsConfig::new(
        vec!["nats://127.0.0.1:4222".to_string()],
        NatsAuth::Credentials(Location::Path("/nonexistent/path/to/creds.nk".into())),
    );

    let err = connect(&config, Duration::from_millis(100))
        .await
        .expect_err("expected an error for missing credentials file");

    assert!(matches!(err, ConnectError::InvalidCredentials(_)));
}

fn config_with(auth: NatsAuth) -> NatsConfig {
    NatsConfig::new(vec!["nats://127.0.0.1:4222".to_string()], auth)
}

fn value(raw: &str) -> CredentialValue {
    CredentialValue::parse(raw).unwrap()
}

#[tokio::test]
async fn build_options_accepts_every_auth_method() {
    let seed = nkeys::KeyPair::new_user().seed().unwrap();
    let seed_file = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(seed_file.path(), format!("{seed}\n")).unwrap();
    let env = InMemoryEnv::new();
    env.set("JWT_SEED", &seed);
    env.set("NKEY_SEED", &seed);

    let methods = [
        NatsAuth::None,
        NatsAuth::UserPassword {
            user: "u".into(),
            password: value("p"),
        },
        NatsAuth::NKey(NKeySeed::parse(&format!("file://{}", seed_file.path().display())).unwrap()),
        NatsAuth::NKey(NKeySeed::parse(&seed).unwrap()),
        NatsAuth::NKey(NKeySeed::parse("env://NKEY_SEED").unwrap()),
        NatsAuth::Token(value("tok")),
        NatsAuth::Jwt {
            jwt: value("jwt"),
            seed: Some(value("env://JWT_SEED")),
        },
        NatsAuth::Jwt {
            jwt: value("jwt"),
            seed: None,
        },
    ];

    for auth in methods {
        build_options(&config_with(auth), &env).await.unwrap();
    }
}

#[tokio::test]
async fn build_options_rejects_unsupported_creds_scheme() {
    let config = config_with(NatsAuth::Credentials(Location::parse("nsc://op/acct/user").unwrap()));

    let err = build_options(&config, &InMemoryEnv::new()).await.unwrap_err();

    assert!(matches!(
        err,
        ConnectError::InvalidSetting {
            setting: "NATS_CREDS",
            error: SettingError::UnsupportedScheme(_)
        }
    ));
}

#[tokio::test]
async fn build_options_rejects_missing_nkey_file() {
    let config = config_with(NatsAuth::NKey(NKeySeed::parse("/nonexistent/seed.nk").unwrap()));

    let err = build_options(&config, &InMemoryEnv::new()).await.unwrap_err();

    assert!(matches!(
        err,
        ConnectError::InvalidSetting {
            setting: "NATS_NKEY",
            error: SettingError::SeedFileRead(_)
        }
    ));
}

#[tokio::test]
async fn build_options_rejects_invalid_seed() {
    let config = config_with(NatsAuth::Jwt {
        jwt: value("jwt"),
        seed: Some(value("not-a-seed")),
    });

    let err = build_options(&config, &InMemoryEnv::new()).await.unwrap_err();

    assert!(matches!(
        err,
        ConnectError::InvalidSetting {
            setting: "NATS_SEED",
            error: SettingError::InvalidSeed(_)
        }
    ));
}

#[tokio::test]
async fn build_options_rejects_unsupported_secret_scheme() {
    let config = config_with(NatsAuth::Token(value("op://vault/item/field")));

    let err = build_options(&config, &InMemoryEnv::new()).await.unwrap_err();

    assert!(matches!(
        err,
        ConnectError::InvalidSetting {
            setting: "NATS_TOKEN",
            ..
        }
    ));
}

#[tokio::test]
async fn build_options_rejects_unset_env_reference() {
    let config = config_with(NatsAuth::Token(value("env://MISSING_TOKEN")));

    let err = build_options(&config, &InMemoryEnv::new()).await.unwrap_err();

    assert!(matches!(
        err,
        ConnectError::InvalidSetting {
            setting: "NATS_TOKEN",
            error: SettingError::EnvUnset(_)
        }
    ));
}

fn client_cert(cert: &str, key: &str) -> Option<ClientCertificate> {
    Some(ClientCertificate {
        cert: Location::parse(cert).unwrap(),
        key: Location::parse(key).unwrap(),
    })
}

fn env_with(pairs: &[(&str, &str)]) -> InMemoryEnv {
    let env = InMemoryEnv::new();
    for (name, value) in pairs {
        env.set(*name, *value);
    }
    env
}

async fn build_options_error(pairs: &[(&str, &str)]) -> ConnectError {
    let env = env_with(pairs);
    build_options(&NatsConfig::from_env(&env), &env).await.unwrap_err()
}

#[tokio::test]
async fn build_options_applies_tls_paths() {
    let tls = NatsTls::Enabled {
        ca: Location::parse("/ca.pem"),
        client_cert: client_cert("/c.pem", "file:///k.pem"),
    };

    build_options(&config_with(NatsAuth::None).with_tls(tls), &InMemoryEnv::new())
        .await
        .unwrap();
}

#[tokio::test]
async fn build_options_applies_client_certificate_without_ca() {
    let tls = NatsTls::Enabled {
        ca: None,
        client_cert: client_cert("/c.pem", "/k.pem"),
    };

    build_options(&config_with(NatsAuth::None).with_tls(tls), &InMemoryEnv::new())
        .await
        .unwrap();
}

#[tokio::test]
async fn build_options_accepts_disabled_tls() {
    build_options(
        &config_with(NatsAuth::None).with_tls(NatsTls::Disabled),
        &InMemoryEnv::new(),
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn build_options_rejects_unsupported_tls_scheme() {
    let tls = NatsTls::Enabled {
        ca: Location::parse("op://vault/ca"),
        client_cert: None,
    };

    let err = build_options(&config_with(NatsAuth::None).with_tls(tls), &InMemoryEnv::new())
        .await
        .unwrap_err();

    assert!(matches!(err, ConnectError::InvalidSetting { setting: "NATS_CA", .. }));
}

#[tokio::test]
async fn build_options_rejects_cert_without_key() {
    let err = build_options_error(&[("NATS_CERT", "/c.pem")]).await;

    assert_eq!(err.to_string(), "Invalid NATS_CERT: must be set together with NATS_KEY");
}

#[tokio::test]
async fn build_options_rejects_key_without_cert() {
    let err = build_options_error(&[("NATS_KEY", "/k.pem"), ("NATS_CA", "/ca.pem")]).await;

    assert_eq!(err.to_string(), "Invalid NATS_KEY: must be set together with NATS_CERT");
}

#[tokio::test]
async fn build_options_rejects_an_unrecognized_tls_disabled_value() {
    let err = build_options_error(&[("NATS_TLS_DISABLED", "false")]).await;

    assert_eq!(
        err.to_string(),
        "Invalid NATS_TLS_DISABLED: `false` is not one of true, 1, yes"
    );
}

#[tokio::test]
async fn build_options_rejects_disabling_tls_alongside_a_tls_setting() {
    let err = build_options_error(&[("NATS_TLS_DISABLED", "true"), ("NATS_CA", "/ca.pem")]).await;

    assert_eq!(
        err.to_string(),
        "Invalid NATS_TLS_DISABLED: cannot be combined with NATS_CA"
    );
}

#[tokio::test]
async fn connect_reports_a_tls_misconfiguration_without_connecting() {
    let env = env_with(&[("NATS_TLS_DISABLED", "off")]);

    let err = connect(&NatsConfig::from_env(&env), Duration::from_millis(100))
        .await
        .unwrap_err();

    assert!(matches!(
        err,
        ConnectError::InvalidSetting {
            setting: "NATS_TLS_DISABLED",
            error: SettingError::UnrecognizedValue { .. }
        }
    ));
}

#[tokio::test]
async fn build_options_rejects_unsupported_client_certificate_scheme() {
    let cert = NatsTls::Enabled {
        ca: None,
        client_cert: client_cert("op://vault/cert", "/k.pem"),
    };
    let key = NatsTls::Enabled {
        ca: None,
        client_cert: client_cert("/c.pem", "op://vault/key"),
    };

    let cert_err = build_options(&config_with(NatsAuth::None).with_tls(cert), &InMemoryEnv::new())
        .await
        .unwrap_err();
    let key_err = build_options(&config_with(NatsAuth::None).with_tls(key), &InMemoryEnv::new())
        .await
        .unwrap_err();

    assert!(matches!(
        cert_err,
        ConnectError::InvalidSetting {
            setting: "NATS_CERT",
            ..
        }
    ));
    assert!(matches!(
        key_err,
        ConnectError::InvalidSetting {
            setting: "NATS_KEY",
            ..
        }
    ));
}

#[tokio::test]
async fn resolve_jwt_drops_surrounding_whitespace() {
    let env = InMemoryEnv::new();
    env.set("USER_JWT", "eyJ0eXAi.payload.sig\n");

    let from_env = resolve_jwt(&value("env://USER_JWT"), &env).await.unwrap();
    let literal = resolve_jwt(&value("eyJ0eXAi.payload.sig\n"), &env).await.unwrap();

    assert_eq!(from_env, "eyJ0eXAi.payload.sig");
    assert_eq!(literal, "eyJ0eXAi.payload.sig");
}

#[tokio::test]
async fn jwt_signer_signs_the_nonce_with_the_seed() {
    let key_pair = Arc::new(KeyPair::new_user());
    let signer = jwt_signer(Arc::clone(&key_pair));

    let signature = signer(b"nonce".to_vec()).await.unwrap();

    key_pair.verify(b"nonce", &signature).unwrap();
}

#[test]
fn connect_error_display_invalid_setting() {
    let err = ConnectError::InvalidSetting {
        setting: "NATS_TOKEN",
        error: SettingError::UnsupportedScheme("op".into()),
    };
    assert_eq!(err.to_string(), "Invalid NATS_TOKEN: scheme `op` is not supported");
}

#[test]
fn connect_error_display_connection_failed_names_no_server() {
    let err = ConnectError::ConnectionFailed {
        servers: vec!["nats://nats.internal.example:4222".to_owned()],
        error: async_nats::ConnectError::new(ConnectErrorKind::TimedOut),
    };

    assert_eq!(err.to_string(), "Failed to connect to NATS: timed out");
}

/// While a single subscriber is registered, `tracing` settles a callsite's interest from whichever
/// thread reaches it first, so a concurrent test that captures nothing would switch the callsite off
/// for the test that does. A second registered subscriber keeps that from happening.
fn keep_another_subscriber_registered(max_level: LevelFilter) -> DefaultGuard {
    CapturedEvents::new().install(max_level)
}

#[tokio::test]
async fn connect_once_logs_the_servers_it_could_not_reach_instead_of_naming_them_in_the_error() {
    let _registered = keep_another_subscriber_registered(LevelFilter::WARN);
    let events = CapturedEvents::new();
    let _guard = events.install(LevelFilter::WARN);
    let config = NatsConfig::from_url(REFUSING_SERVER).with_tls(NatsTls::Disabled);

    let err = connect_once(&config, NatsAuth::None, REFUSED_DIAL_TIMEOUT)
        .await
        .unwrap_err();

    let ConnectError::ConnectionFailed { servers, error } = &err else {
        panic!("expected a connection failure, got {err:?}");
    };
    assert_eq!(*servers, config.servers);
    assert_eq!(err.to_string(), format!("Failed to connect to NATS: {error}"));
    let failure = events
        .events()
        .into_iter()
        .find(|event| event.message() == Some(CONNECT_FAILURE_WARNING))
        .expect("the failure is logged where the connection was made");
    assert!(
        failure
            .field("servers")
            .is_some_and(|servers| servers.contains(REFUSING_SERVER)),
        "{failure:?}"
    );
}

async fn connect_once_from(env: &InMemoryEnv) -> Result<Client, ConnectError> {
    let config = NatsConfig::from_env(env);
    connect_once(&config, config.auth.clone(), PLAINTEXT_CONNECT_TIMEOUT).await
}

fn connection_failure(err: ConnectError) -> async_nats::ConnectError {
    match err {
        ConnectError::ConnectionFailed { error, .. } => error,
        other => panic!("expected a connection failure, got {other:?}"),
    }
}

fn token(raw: &str) -> NatsAuth {
    NatsAuth::Token(CredentialValue::Literal(SecretString::new(raw).unwrap()))
}

fn token_server_config(server: &CoreTestServer, auth: NatsAuth) -> NatsConfig {
    NatsConfig::new(vec![server.address().to_owned()], auth).with_tls(NatsTls::Disabled)
}

fn handshake_error(err: &async_nats::ConnectError) -> Option<&async_nats::rustls::Error> {
    std::error::Error::source(err)?
        .downcast_ref::<std::io::Error>()?
        .get_ref()?
        .downcast_ref::<async_nats::rustls::Error>()
}

#[tokio::test]
async fn default_settings_cannot_connect_to_a_plaintext_server() {
    let server = CoreTestServer::start().await;
    let env = env_with(&[("NATS_URL", server.address())]);

    let err = connection_failure(connect_once_from(&env).await.unwrap_err());

    assert!(
        matches!(
            handshake_error(&err),
            Some(async_nats::rustls::Error::InvalidMessage(_))
        ),
        "{err:?}"
    );
}

#[tokio::test]
async fn disabling_tls_connects_to_a_plaintext_server() {
    let server = CoreTestServer::start().await;
    let env = env_with(&[("NATS_URL", server.address()), ("NATS_TLS_DISABLED", "true")]);

    let client = connect_once_from(&env).await.unwrap();

    client.flush().await.unwrap();
}

#[tokio::test]
async fn connect_once_reports_a_tls_misconfiguration_without_connecting() {
    let env = env_with(&[("NATS_TLS_DISABLED", "off")]);

    let err = connect_once_from(&env).await.unwrap_err();

    assert!(matches!(
        err,
        ConnectError::InvalidSetting {
            setting: "NATS_TLS_DISABLED",
            error: SettingError::UnrecognizedValue { .. }
        }
    ));
}

#[tokio::test]
async fn connect_once_fails_after_one_pass_instead_of_retrying() {
    let config = NatsConfig::from_url(REFUSING_SERVER).with_tls(NatsTls::Disabled);

    let err = tokio::time::timeout(
        FAIL_FAST_DEADLINE,
        connect_once(&config, NatsAuth::None, REFUSED_DIAL_TIMEOUT),
    )
    .await
    .expect("connect_once gives up after one pass over the servers")
    .unwrap_err();

    assert!(matches!(err, ConnectError::ConnectionFailed { .. }), "{err:?}");
}

#[tokio::test]
async fn connect_once_reaches_a_live_server_listed_after_an_unreachable_one() {
    let server = CoreTestServer::start().await;
    let config = NatsConfig::new(
        vec![REFUSING_SERVER.to_owned(), server.address().to_owned()],
        NatsAuth::None,
    )
    .with_tls(NatsTls::Disabled);

    // The servers are tried in a fresh random order on every pass, so run enough
    // passes for the unreachable one to come first in at least one of them.
    for _ in 0..SHUFFLED_PASSES {
        let client = connect_once(&config, NatsAuth::None, REFUSED_DIAL_TIMEOUT)
            .await
            .unwrap();

        client.flush().await.unwrap();
    }
}

async fn wait_until_disconnected(client: &Client) {
    tokio::time::timeout(FAIL_FAST_DEADLINE, async {
        while client.connection_state() != State::Disconnected {
            tokio::time::sleep(SERVER_INFO_POLL_INTERVAL).await;
        }
    })
    .await
    .expect("the client notices that its server is gone");
}

#[tokio::test]
async fn connect_once_closes_for_good_after_one_pass_once_its_connection_drops() {
    let server = CoreTestServer::start().await;
    let config = NatsConfig::from_url(server.address()).with_tls(NatsTls::Disabled);
    let client = connect_once(&config, NatsAuth::None, REFUSED_DIAL_TIMEOUT)
        .await
        .unwrap();
    client.flush().await.unwrap();

    drop(server);
    wait_until_disconnected(&client).await;

    tokio::time::timeout(FAIL_FAST_DEADLINE, client.flush())
        .await
        .expect("a dropped connection gives up after one pass instead of redialing forever")
        .unwrap_err();
}

#[tokio::test]
async fn connect_once_authenticates_with_the_auth_it_is_given() {
    let server = CoreTestServer::start_requiring_token(SERVER_TOKEN).await;
    let config = token_server_config(&server, token(OTHER_TOKEN));

    let client = connect_once(&config, token(SERVER_TOKEN), PLAINTEXT_CONNECT_TIMEOUT)
        .await
        .unwrap();

    client.flush().await.unwrap();
}

#[tokio::test]
async fn connect_once_returns_a_rejected_token_as_an_authorization_violation() {
    let server = CoreTestServer::start_requiring_token(SERVER_TOKEN).await;
    let config = token_server_config(&server, token(SERVER_TOKEN));

    let err = connect_once(&config, token(OTHER_TOKEN), PLAINTEXT_CONNECT_TIMEOUT)
        .await
        .unwrap_err();

    assert_eq!(connection_failure(err).kind(), ConnectErrorKind::AuthorizationViolation);
}

#[tokio::test]
async fn connect_once_rejects_an_empty_server_list() {
    let config = NatsConfig::new(Vec::new(), NatsAuth::None);

    let err = connect_once(&config, NatsAuth::None, PLAINTEXT_CONNECT_TIMEOUT)
        .await
        .unwrap_err();

    assert!(matches!(err, ConnectError::NoServers), "{err:?}");
}

#[tokio::test]
async fn connect_rejects_a_server_list_that_names_no_server() {
    let env = env_with(&[("NATS_URL", " , ")]);

    let err = connect(&NatsConfig::from_env(&env), PLAINTEXT_CONNECT_TIMEOUT)
        .await
        .unwrap_err();

    assert!(matches!(err, ConnectError::NoServers), "{err:?}");
}

#[tokio::test]
async fn connect_dials_the_first_server_without_waiting() {
    let server = CoreTestServer::start().await;
    let config = NatsConfig::from_url(server.address()).with_tls(NatsTls::Disabled);

    let client = connect(&config, PLAINTEXT_CONNECT_TIMEOUT).await.unwrap();

    wait_for_server_info(&client, FIRST_DIAL_DEADLINE, SERVER_INFO_POLL_INTERVAL)
        .await
        .unwrap();
}

fn tls_disabled_warnings(events: &CapturedEvents) -> usize {
    events
        .events()
        .iter()
        .filter(|event| event.message() == Some(TLS_DISABLED_WARNING))
        .count()
}

#[tokio::test]
async fn connect_warns_once_when_tls_is_disabled() {
    let events = CapturedEvents::new();
    let _guard = events.install(LevelFilter::WARN);
    let env = env_with(&[("NATS_URL", "127.0.0.1:1"), ("NATS_TLS_DISABLED", "true")]);

    connect(&NatsConfig::from_env(&env), Duration::from_millis(100))
        .await
        .unwrap();

    assert_eq!(tls_disabled_warnings(&events), 1);
}

#[tokio::test]
async fn connect_does_not_warn_when_tls_is_on() {
    let events = CapturedEvents::new();
    let _guard = events.install(LevelFilter::WARN);
    let env = env_with(&[("NATS_URL", "127.0.0.1:1")]);

    connect(&NatsConfig::from_env(&env), Duration::from_millis(100))
        .await
        .unwrap();

    assert_eq!(tls_disabled_warnings(&events), 0);
}

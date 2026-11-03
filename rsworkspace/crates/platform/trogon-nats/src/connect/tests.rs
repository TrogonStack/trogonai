use async_nats::{ClientError, ServerError};

use super::*;
use crate::credential_value::{CredentialValue, Location, NKeySeed};
use trogon_std::env::InMemoryEnv;

#[test]
fn test_reconnect_delay_starts_at_one_second() {
    assert_eq!(reconnect_delay(0).as_secs(), 1);
}

#[test]
fn test_reconnect_delay_exponential_backoff() {
    assert_eq!(reconnect_delay(0).as_secs(), 1);
    assert_eq!(reconnect_delay(1).as_secs(), 2);
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

#[tokio::test]
async fn build_options_applies_tls_paths() {
    let tls = NatsTls::new(
        Location::parse("/c.pem"),
        Location::parse("file:///k.pem"),
        Location::parse("/ca.pem"),
    );

    build_options(&config_with(NatsAuth::None).with_tls(tls), &InMemoryEnv::new())
        .await
        .unwrap();
}

#[tokio::test]
async fn build_options_applies_client_certificate_without_ca() {
    let tls = NatsTls::new(Location::parse("/c.pem"), Location::parse("/k.pem"), None);

    build_options(&config_with(NatsAuth::None).with_tls(tls), &InMemoryEnv::new())
        .await
        .unwrap();
}

#[tokio::test]
async fn build_options_rejects_unsupported_tls_scheme() {
    let tls = NatsTls::new(None, None, Location::parse("op://vault/ca"));

    let err = build_options(&config_with(NatsAuth::None).with_tls(tls), &InMemoryEnv::new())
        .await
        .unwrap_err();

    assert!(matches!(err, ConnectError::InvalidSetting { setting: "NATS_CA", .. }));
}

#[tokio::test]
async fn build_options_rejects_cert_without_key() {
    let tls = NatsTls::new(Location::parse("/c.pem"), None, None);

    let err = build_options(&config_with(NatsAuth::None).with_tls(tls), &InMemoryEnv::new())
        .await
        .unwrap_err();

    assert_eq!(err.to_string(), "Invalid NATS_CERT: must be set together with NATS_KEY");
}

#[tokio::test]
async fn build_options_rejects_key_without_cert() {
    let tls = NatsTls::new(None, Location::parse("/k.pem"), Location::parse("/ca.pem"));

    let err = build_options(&config_with(NatsAuth::None).with_tls(tls), &InMemoryEnv::new())
        .await
        .unwrap_err();

    assert_eq!(err.to_string(), "Invalid NATS_KEY: must be set together with NATS_CERT");
}

#[tokio::test]
async fn build_options_rejects_unsupported_client_certificate_scheme() {
    let cert = NatsTls::new(Location::parse("op://vault/cert"), Location::parse("/k.pem"), None);
    let key = NatsTls::new(Location::parse("/c.pem"), Location::parse("op://vault/key"), None);

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

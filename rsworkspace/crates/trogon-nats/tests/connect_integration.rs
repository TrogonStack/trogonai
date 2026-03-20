//! Integration tests for `trogon_nats::connect` — requires Docker (testcontainers starts NATS).

use std::time::Duration;
use testcontainers_modules::nats::Nats;
use testcontainers_modules::testcontainers::runners::AsyncRunner;
use trogon_nats::auth::{NatsAuth, NatsConfig};
use trogon_nats::connect::{ConnectError, connect};

async fn start_nats() -> (
    testcontainers_modules::testcontainers::ContainerAsync<Nats>,
    u16,
) {
    let container = Nats::default()
        .start()
        .await
        .expect("Failed to start NATS container — is Docker running?");
    let port = container.get_host_port_ipv4(4222).await.unwrap();
    (container, port)
}

/// Covers the `NatsAuth::None` arm (lines 123-128) and the success branch (130-138).
/// Also exercises `apply_reconnect_options` (lines 69-74) indirectly.
#[tokio::test]
async fn connect_with_no_auth_succeeds() {
    let (_container, port) = start_nats().await;

    let config = NatsConfig::new(vec![format!("nats://127.0.0.1:{port}")], NatsAuth::None);

    let _client = connect(&config, Duration::from_secs(10))
        .await
        .expect("connect() should succeed with a running NATS server");
    // client drops here → connection closes
}

/// Covers the `NatsAuth::Token` arm (lines 115-122).
#[tokio::test]
async fn connect_with_token_auth_succeeds_on_open_server() {
    // An open NATS server accepts any token — the token is just passed through.
    let (_container, port) = start_nats().await;

    let config = NatsConfig::new(
        vec![format!("nats://127.0.0.1:{port}")],
        NatsAuth::Token("any-token".to_string()),
    );

    let _client = connect(&config, Duration::from_secs(10))
        .await
        .expect("open NATS server should accept connections regardless of token");
}

/// Covers the `NatsAuth::UserPassword` arm (lines 107-114).
#[tokio::test]
async fn connect_with_user_password_succeeds_on_open_server() {
    let (_container, port) = start_nats().await;

    let config = NatsConfig::new(
        vec![format!("nats://127.0.0.1:{port}")],
        NatsAuth::UserPassword {
            user: "user".to_string(),
            password: "pass".to_string(),
        },
    );

    let _client = connect(&config, Duration::from_secs(10))
        .await
        .expect("open NATS server should accept user/password connections");
}

/// Covers the `NatsAuth::Credentials` arm — specifically the `InvalidCredentials`
/// error path (lines 88-100) when the credentials file does not exist.
/// No Docker required: the error is returned before any network activity.
#[tokio::test]
async fn connect_with_missing_credentials_file_returns_invalid_credentials() {
    let config = NatsConfig::new(
        vec!["nats://127.0.0.1:4222".to_string()],
        NatsAuth::Credentials("/nonexistent/path/trogon_test_creds.creds".into()),
    );

    let result = connect(&config, Duration::from_secs(5)).await;

    assert!(
        matches!(result, Err(ConnectError::InvalidCredentials(_))),
        "expected InvalidCredentials, got: {:?}",
        result
    );
}

//! Integration tests for `trogon_nats::connect` — requires Docker (testcontainers starts NATS).
//!
//! Tests that need Docker are marked `#[ignore]` and only run when explicitly
//! requested:
//!
//! ```sh
//! cargo test -p trogon-nats -- --ignored
//! ```

use std::time::Duration;
use testcontainers_modules::nats::Nats;
use testcontainers_modules::testcontainers::ContainerAsync;
use testcontainers_modules::testcontainers::ImageExt;
use testcontainers_modules::testcontainers::runners::AsyncRunner;
use trogon_nats::auth::{NatsAuth, NatsConfig};
use trogon_nats::connect::{ConnectError, connect};

async fn start_nats() -> Result<(ContainerAsync<Nats>, String, u16), Box<dyn std::error::Error>> {
    let container = Nats::default().start().await?;
    let host = container.get_host().await?.to_string();
    let port = container.get_host_port_ipv4(4222).await?;
    Ok((container, host, port))
}

/// Covers the `NatsAuth::None` arm (lines 123-128) and the success branch (130-138).
/// Also exercises `apply_reconnect_options` (lines 69-74) indirectly.
#[tokio::test]
#[ignore = "requires Docker"]
async fn connect_with_no_auth_succeeds() -> Result<(), Box<dyn std::error::Error>> {
    let (_container, host, port) = start_nats().await?;

    let config = NatsConfig::new(vec![format!("nats://{host}:{port}")], NatsAuth::None);

    connect(&config, Duration::from_secs(10))
        .await
        .expect("connect() should succeed with a running NATS server");
    Ok(())
}

/// Covers the `NatsAuth::Token` arm (lines 115-122).
#[tokio::test]
#[ignore = "requires Docker"]
async fn connect_with_token_auth_succeeds_on_open_server() -> Result<(), Box<dyn std::error::Error>>
{
    // An open NATS server accepts any token — the token is just passed through.
    let (_container, host, port) = start_nats().await?;

    let config = NatsConfig::new(
        vec![format!("nats://{host}:{port}")],
        NatsAuth::Token("any-token".to_string()),
    );

    connect(&config, Duration::from_secs(10))
        .await
        .expect("open NATS server should accept connections regardless of token");
    Ok(())
}

/// Covers the `NatsAuth::UserPassword` arm (lines 107-114).
#[tokio::test]
#[ignore = "requires Docker"]
async fn connect_with_user_password_succeeds_on_open_server()
-> Result<(), Box<dyn std::error::Error>> {
    let (_container, host, port) = start_nats().await?;

    let config = NatsConfig::new(
        vec![format!("nats://{host}:{port}")],
        NatsAuth::UserPassword {
            user: "user".to_string(),
            password: "pass".to_string(),
        },
    );

    connect(&config, Duration::from_secs(10))
        .await
        .expect("open NATS server should accept user/password connections");
    Ok(())
}

/// Covers the `NatsAuth::NKey` arm (lines 101-106).
///
/// async_nats sends the NKey challenge-response during the CONNECT handshake.
/// An open NATS server (no `authorization` config) does not enforce auth and
/// accepts the connection regardless of which key is presented.
#[tokio::test]
#[ignore = "requires Docker"]
async fn connect_with_nkey_auth_on_open_server() -> Result<(), Box<dyn std::error::Error>> {
    let (_container, host, port) = start_nats().await?;

    // A valid NKey user seed (base32-encoded, 58-char canonical format,
    // starts with "SU"). On an open server the key is not validated against
    // a registered user — the test simply exercises the `NatsAuth::NKey`
    // branch in `connect()`.
    let seed = "SUANQDPB2RUOE4ETUA26CNX7FUKE5ZZKFCQIIW63OX225F2CO7UEXTM7ZY".to_string();

    let config = NatsConfig::new(vec![format!("nats://{host}:{port}")], NatsAuth::NKey(seed));

    let result = connect(&config, Duration::from_secs(10)).await;
    assert!(
        result.is_ok(),
        "NKey connect should succeed on an open NATS server: {:?}",
        result
    );
    Ok(())
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

/// Wrong token against an auth-enabled NATS server must return
/// `ConnectError::AuthorizationViolation` immediately instead of retrying forever.
#[tokio::test]
#[ignore = "requires Docker"]
async fn connect_with_wrong_token_returns_authorization_violation()
-> Result<(), Box<dyn std::error::Error>> {
    let container = Nats::default()
        .with_cmd(["--auth", "correct-token"])
        .start()
        .await?;
    let host = container.get_host().await?.to_string();
    let port = container.get_host_port_ipv4(4222).await?;

    let config = NatsConfig::new(
        vec![format!("nats://{host}:{port}")],
        NatsAuth::Token("wrong-token".to_string()),
    );

    let result = connect(&config, Duration::from_secs(10)).await;

    assert!(
        matches!(result, Err(ConnectError::AuthorizationViolation)),
        "expected AuthorizationViolation, got: {:?}",
        result
    );
    Ok(())
}

/// Correct token must still connect successfully after the fix.
#[tokio::test]
#[ignore = "requires Docker"]
async fn connect_with_correct_token_succeeds() -> Result<(), Box<dyn std::error::Error>> {
    let container = Nats::default()
        .with_startup_timeout(Duration::from_secs(30))
        .with_cmd(["--auth", "correct-token"])
        .start()
        .await?;
    let host = container.get_host().await?.to_string();
    let port = container.get_host_port_ipv4(4222).await?;

    let config = NatsConfig::new(
        vec![format!("nats://{host}:{port}")],
        NatsAuth::Token("correct-token".to_string()),
    );

    let result = connect(&config, Duration::from_secs(10)).await;
    assert!(
        result.is_ok(),
        "correct token should connect successfully: {:?}",
        result
    );
    Ok(())
}

/// Covers the `_ = tokio::time::sleep(check_window)` arm in `connect()`.
///
/// When the server is unreachable, no `Connected` or auth-violation event fires
/// within `INITIAL_CONNECT_CHECK_SECS`. The select times out and `connect()`
/// returns `Ok(client)` so the caller's retry loop can continue in the background.
/// No Docker required: we simply point at a port with nothing listening.
#[tokio::test]
async fn connect_to_unreachable_server_returns_ok_with_background_retry() {
    // Bind to port 0 to get a free ephemeral port, then immediately drop the
    // listener so nothing is listening — avoids hard-coded port collisions.
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);

    let config = NatsConfig::new(vec![format!("nats://127.0.0.1:{port}")], NatsAuth::None);

    // connect() must return within a few seconds (INITIAL_CONNECT_CHECK_SECS + margin).
    let result = tokio::time::timeout(
        Duration::from_secs(10),
        connect(&config, Duration::from_secs(30)),
    )
    .await
    .expect("connect() must not hang indefinitely on unreachable server");

    assert!(
        result.is_ok(),
        "expected Ok(client) for unreachable server (retry in background), got: {:?}",
        result
    );
}

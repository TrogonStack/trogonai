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
    let err = ConnectError::InvalidCredentials(std::io::Error::new(std::io::ErrorKind::NotFound, "file not found"));
    let msg = err.to_string();
    assert!(msg.contains("Failed to load credentials file"));
    assert!(msg.contains("file not found"));
}

#[test]
fn connect_error_source_invalid_credentials() {
    let err = ConnectError::InvalidCredentials(std::io::Error::new(std::io::ErrorKind::NotFound, "file not found"));
    assert!(std::error::Error::source(&err).is_some());
}

/// `ConnectError::ConnectionFailed` display must include both the server
/// list and the inner error message.  Source must return `Some`.
///
/// We obtain a real `async_nats::ConnectError` by attempting to connect to
/// a port that is almost certainly not listening, **without**
/// `retry_on_initial_connect()` so the call returns immediately.
#[tokio::test]
async fn connect_error_connection_failed_display_and_source() {
    // Use a port in the ephemeral range that is unlikely to be in use.
    let bad_addr = "nats://127.0.0.1:19122";

    // Call async_nats directly — without retry_on_initial_connect() the
    // connection attempt fails immediately when the server is absent.
    let result = async_nats::ConnectOptions::new().connect(bad_addr).await;

    match result {
        Err(nats_err) => {
            let err = ConnectError::ConnectionFailed {
                servers: vec![bad_addr.to_string()],
                error: nats_err,
            };
            let msg = err.to_string();
            assert!(
                msg.contains("Failed to connect to NATS servers"),
                "display must mention the failure: {}",
                msg
            );
            assert!(msg.contains("19122"), "display must include the server: {}", msg);
            assert!(
                std::error::Error::source(&err).is_some(),
                "source() must expose the inner async_nats error"
            );
        }
        Ok(_) => {
            // A server happened to be running on port 19122; skip.
        }
    }
}

/// Display for `ConnectionFailed` must include the server list and the
/// underlying async-nats error message.
#[tokio::test]
async fn connect_error_display_connection_failed() {
    let raw_err = async_nats::connect("nats://127.0.0.1:1")
        .await
        .expect_err("expected connection refused on port 1");

    let servers = vec!["nats://127.0.0.1:1".to_string()];
    let err = ConnectError::ConnectionFailed {
        servers: servers.clone(),
        error: raw_err,
    };

    let msg = err.to_string();
    assert!(
        msg.contains("Failed to connect to NATS servers"),
        "display missing prefix; got: {msg}"
    );
    assert!(msg.contains("127.0.0.1:1"), "display missing server; got: {msg}");
}

/// `source()` on `ConnectionFailed` must return `Some`.
#[tokio::test]
async fn connect_error_source_connection_failed() {
    let raw_err = async_nats::connect("nats://127.0.0.1:1")
        .await
        .expect_err("expected connection refused on port 1");

    let err = ConnectError::ConnectionFailed {
        servers: vec!["nats://127.0.0.1:1".to_string()],
        error: raw_err,
    };

    assert!(
        std::error::Error::source(&err).is_some(),
        "ConnectionFailed must expose a source error"
    );
}

/// Calling `connect()` with `NatsAuth::Credentials` pointing to a
/// non-existent file must return `ConnectError::InvalidCredentials`
/// (exercises the `Err(e)` branch of `with_credentials_file()`).
#[tokio::test]
async fn connect_with_missing_credentials_file_returns_invalid_credentials() {
    let config = NatsConfig::new(
        vec!["nats://127.0.0.1:4222".to_string()],
        NatsAuth::Credentials("/nonexistent/path/to/creds.nk".into()),
    );

    let err = connect(&config, Duration::from_millis(100))
        .await
        .expect_err("expected an error for missing credentials file");

    assert!(
        matches!(err, ConnectError::InvalidCredentials(_)),
        "missing credentials file must produce ConnectError::InvalidCredentials; got: {err}"
    );
    assert!(
        err.to_string().contains("Failed to load credentials file"),
        "error message must mention credentials file; got: {err}"
    );
}

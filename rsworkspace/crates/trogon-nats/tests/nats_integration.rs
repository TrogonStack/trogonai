//! Integration tests for trogon-nats with a real NATS server.
//!
//! Requires Docker (uses testcontainers to spin up a NATS server).
//!
//! Run with:
//!   cargo test -p trogon-nats --test nats_integration

use std::time::Duration;

use futures_util::StreamExt as _;
use testcontainers_modules::nats::Nats;
use testcontainers_modules::testcontainers::{ContainerAsync, ImageExt, runners::AsyncRunner};
use trogon_nats::{
    ConnectError, FlushClient, NatsAuth, NatsConfig, PublishClient, PublishOptions,
    SubscribeClient, connect, publish, request_with_timeout,
};

// ── Helpers ───────────────────────────────────────────────────────────────────

async fn start_nats() -> (ContainerAsync<Nats>, u16) {
    let container = Nats::default()
        .start()
        .await
        .expect("Failed to start NATS container — is Docker running?");
    let port = container.get_host_port_ipv4(4222).await.unwrap();
    (container, port)
}

fn nats_config(port: u16) -> NatsConfig {
    NatsConfig {
        servers: vec![format!("127.0.0.1:{port}")],
        auth: NatsAuth::None,
    }
}

async fn nats_client(port: u16) -> async_nats::Client {
    async_nats::connect(format!("127.0.0.1:{port}"))
        .await
        .expect("Failed to connect to NATS")
}

// ── Tests ─────────────────────────────────────────────────────────────────────

/// `trogon_nats::connect()` returns a working client when the server is reachable.
#[tokio::test]
async fn connect_to_nats_succeeds() {
    let (_container, port) = start_nats().await;
    let config = nats_config(port);

    let result = connect(&config, Duration::from_secs(5)).await;
    assert!(
        result.is_ok(),
        "expected successful connection, got: {:?}",
        result.unwrap_err()
    );

    // Verify the client is functional by publishing a message.
    let client = result.unwrap();
    let pub_result = PublishClient::publish_with_headers(
        &client,
        "test.subject",
        async_nats::HeaderMap::new(),
        b"ping".as_ref().into(),
    )
    .await;
    assert!(pub_result.is_ok());
}

/// `publish()` sends a message that a subscriber can receive.
#[tokio::test]
async fn publish_and_subscribe_roundtrip() {
    let (_container, port) = start_nats().await;
    let nats = nats_client(port).await;

    let mut sub = SubscribeClient::subscribe(&nats, "roundtrip.subject")
        .await
        .unwrap();

    let payload = serde_json::json!({ "hello": "world" });
    publish(
        &nats,
        "roundtrip.subject",
        &payload,
        PublishOptions::simple(),
    )
    .await
    .unwrap();

    let msg = tokio::time::timeout(Duration::from_secs(2), sub.next())
        .await
        .expect("timed out waiting for message")
        .expect("subscriber closed");

    let received: serde_json::Value = serde_json::from_slice(&msg.payload).unwrap();
    assert_eq!(received, payload);
}

/// `request_with_timeout()` gets a response from a mock responder.
#[tokio::test]
async fn request_reply_roundtrip() {
    let (_container, port) = start_nats().await;
    let nats = nats_client(port).await;

    let mut sub = nats.subscribe("request.subject").await.unwrap();
    let nats2 = nats.clone();
    tokio::spawn(async move {
        if let Some(msg) = sub.next().await
            && let Some(reply) = msg.reply
        {
            let resp = serde_json::to_vec(&serde_json::json!({ "status": "ok" })).unwrap();
            nats2.publish(reply, resp.into()).await.unwrap();
        }
    });

    let result: Result<serde_json::Value, _> = request_with_timeout(
        &nats,
        "request.subject",
        &serde_json::json!({ "query": "ping" }),
        Duration::from_secs(2),
    )
    .await;

    assert!(
        result.is_ok(),
        "expected Ok, got: {:?}",
        result.unwrap_err()
    );
    let response = result.unwrap();
    assert_eq!(response["status"], "ok");
}

/// `request_with_timeout()` returns an error when nobody is subscribed.
///
/// NATS immediately returns "no responders" when no subscriber exists — there
/// is no need to wait for the timeout in this case.
#[tokio::test]
async fn request_times_out_when_no_responder() {
    let (_container, port) = start_nats().await;
    let nats = nats_client(port).await;

    let result: Result<serde_json::Value, _> = request_with_timeout(
        &nats,
        "no-responder.subject",
        &serde_json::json!({}),
        Duration::from_millis(300),
    )
    .await;

    assert!(result.is_err());
    // NATS returns "no responders" immediately when nobody is subscribed.
    // This surfaces as a Request error rather than a Timeout.
    match result.unwrap_err() {
        trogon_nats::NatsError::Request { subject, error } => {
            assert_eq!(subject, "no-responder.subject");
            assert!(
                error.contains("no responders"),
                "expected 'no responders' in error, got: {error}"
            );
        }
        e => panic!("expected Request(no responders) error, got: {:?}", e),
    }
}

/// Cloned NATS clients share the same connection and can both publish.
#[tokio::test]
async fn multiple_clients_share_connection() {
    let (_container, port) = start_nats().await;
    let nats1 = nats_client(port).await;
    let nats2 = nats1.clone(); // Same underlying connection.

    let mut sub = nats1.subscribe("shared.>").await.unwrap();

    publish(
        &nats1,
        "shared.from-client-1",
        &serde_json::json!("msg1"),
        PublishOptions::simple(),
    )
    .await
    .unwrap();

    publish(
        &nats2,
        "shared.from-client-2",
        &serde_json::json!("msg2"),
        PublishOptions::simple(),
    )
    .await
    .unwrap();

    let mut received_subjects = Vec::new();
    for _ in 0..2 {
        let msg = tokio::time::timeout(Duration::from_secs(2), sub.next())
            .await
            .expect("timed out")
            .expect("subscriber closed");
        received_subjects.push(msg.subject.to_string());
    }

    assert!(received_subjects.contains(&"shared.from-client-1".to_string()));
    assert!(received_subjects.contains(&"shared.from-client-2".to_string()));
}

/// `FlushClient::flush()` completes without error after a publish.
#[tokio::test]
async fn flush_after_publish() {
    let (_container, port) = start_nats().await;
    let nats = nats_client(port).await;

    // Publish several messages then flush.
    for i in 0..3 {
        publish(
            &nats,
            &format!("flush.test.{}", i),
            &serde_json::json!({ "i": i }),
            PublishOptions::simple(),
        )
        .await
        .unwrap();
    }

    let flush_result = FlushClient::flush(&nats).await;
    assert!(
        flush_result.is_ok(),
        "flush should succeed, got: {:?}",
        flush_result.unwrap_err()
    );
}

/// `connect()` returns `InvalidCredentials` when the credentials file does not exist.
///
/// `trogon_nats::connect()` uses `retry_on_initial_connect()` so bad ports
/// never fail — they retry until `connection_timeout`. Testing with a
/// non-existent credentials file exercises the `ConnectError::InvalidCredentials`
/// path, which fails immediately before any TCP attempt.
#[tokio::test]
async fn connect_fails_on_invalid_credentials_file() {
    let config = NatsConfig {
        servers: vec!["127.0.0.1:4222".to_string()],
        auth: NatsAuth::Credentials("/nonexistent/path/credentials.creds".into()),
    };

    let result = connect(&config, Duration::from_secs(5)).await;
    assert!(
        result.is_err(),
        "expected error for non-existent credentials file"
    );
    assert!(
        matches!(result.unwrap_err(), ConnectError::InvalidCredentials(_)),
        "expected InvalidCredentials variant"
    );
}

/// `NatsAuth::Token` connects successfully when the server requires a matching token.
#[tokio::test]
async fn connect_with_token_auth_succeeds() {
    let token = "secret-nats-token";
    let container = Nats::default()
        .with_cmd(["--auth", token])
        .start()
        .await
        .expect("Failed to start NATS container");
    let port = container.get_host_port_ipv4(4222).await.unwrap();

    let config = NatsConfig::new(
        vec![format!("127.0.0.1:{port}")],
        NatsAuth::Token(token.to_string()),
    );

    let result = connect(&config, Duration::from_secs(5)).await;
    assert!(
        result.is_ok(),
        "Token auth must succeed with correct token; got: {:?}",
        result.unwrap_err()
    );

    // Verify the client is functional.
    let client = result.unwrap();
    let pub_result = PublishClient::publish_with_headers(
        &client,
        "auth.token.test",
        async_nats::HeaderMap::new(),
        b"hello".as_ref().into(),
    )
    .await;
    assert!(pub_result.is_ok(), "publish after token auth must succeed");
}

/// `NatsAuth::UserPassword` connects successfully when the server requires
/// matching user/password credentials.
#[tokio::test]
async fn connect_with_user_password_auth_succeeds() {
    let user = "trogon";
    let password = "s3cret";
    let container = Nats::default()
        .with_cmd(["--user", user, "--pass", password])
        .start()
        .await
        .expect("Failed to start NATS container");
    let port = container.get_host_port_ipv4(4222).await.unwrap();

    let config = NatsConfig::new(
        vec![format!("127.0.0.1:{port}")],
        NatsAuth::UserPassword {
            user: user.to_string(),
            password: password.to_string(),
        },
    );

    let result = connect(&config, Duration::from_secs(5)).await;
    assert!(
        result.is_ok(),
        "UserPassword auth must succeed with correct credentials; got: {:?}",
        result.unwrap_err()
    );

    let client = result.unwrap();
    let pub_result = PublishClient::publish_with_headers(
        &client,
        "auth.userpass.test",
        async_nats::HeaderMap::new(),
        b"hello".as_ref().into(),
    )
    .await;
    assert!(
        pub_result.is_ok(),
        "publish after user/password auth must succeed"
    );
}

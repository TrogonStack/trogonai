//! Integration tests for trogon_nats::messaging — requires Docker (testcontainers starts NATS).
//!
//! These tests exercise `publish`, `request`, and `request_with_timeout` against a real
//! NATS server (started via testcontainers) to complement the unit tests that use mocks.

use futures::StreamExt;
use serde::{Deserialize, Serialize};
use std::time::Duration;
use testcontainers_modules::nats::Nats;
use testcontainers_modules::testcontainers::runners::AsyncRunner;
use trogon_nats::{
    FlushPolicy, NatsAuth, NatsConfig, NatsError, PublishOptions, connect, publish, request,
    request_with_timeout,
};

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

async fn nats_client(port: u16) -> async_nats::Client {
    let config = NatsConfig::new(vec![format!("nats://127.0.0.1:{port}")], NatsAuth::None);
    connect(&config, Duration::from_secs(10))
        .await
        .expect("connect should succeed")
}

#[derive(Serialize, Deserialize, Debug, PartialEq)]
struct Ping {
    value: u32,
}

#[derive(Serialize, Deserialize, Debug, PartialEq)]
struct Pong {
    echoed: u32,
}

/// `publish()` with no flush option delivers the message to a subscriber.
#[tokio::test]
async fn publish_delivers_to_subscriber() {
    let (_container, port) = start_nats().await;
    let client = nats_client(port).await;

    let mut sub = client.subscribe("test.msg.publish").await.unwrap();

    publish(
        &client,
        "test.msg.publish",
        &Ping { value: 42 },
        PublishOptions::simple(),
    )
    .await
    .expect("publish should succeed");

    let msg = tokio::time::timeout(Duration::from_secs(5), sub.next())
        .await
        .expect("timeout waiting for message")
        .expect("expected a message");

    let received: Ping = serde_json::from_slice(&msg.payload).unwrap();
    assert_eq!(received.value, 42);
}

/// `publish()` with `FlushPolicy` flushes to the server and the message is still received.
#[tokio::test]
async fn publish_with_flush_delivers_to_subscriber() {
    let (_container, port) = start_nats().await;
    let client = nats_client(port).await;

    let mut sub = client.subscribe("test.msg.publish_flush").await.unwrap();

    let options = PublishOptions::builder()
        .flush_policy(FlushPolicy::no_retries())
        .build();

    publish(
        &client,
        "test.msg.publish_flush",
        &Ping { value: 99 },
        options,
    )
    .await
    .expect("publish with flush should succeed");

    let msg = tokio::time::timeout(Duration::from_secs(5), sub.next())
        .await
        .expect("timeout waiting for message")
        .expect("expected a message");

    let received: Ping = serde_json::from_slice(&msg.payload).unwrap();
    assert_eq!(received.value, 99);
}

/// `request()` completes a full round-trip when a responder is running.
#[tokio::test]
async fn request_receives_reply() {
    let (_container, port) = start_nats().await;
    let client = nats_client(port).await;

    // Spawn a responder that echoes the value back.
    let mut sub = client.subscribe("test.msg.request").await.unwrap();
    let responder = client.clone();
    tokio::spawn(async move {
        if let Some(msg) = sub.next().await {
            if let Some(reply) = msg.reply {
                let req: Ping = serde_json::from_slice(&msg.payload).unwrap();
                let pong = Pong { echoed: req.value };
                let payload = serde_json::to_vec(&pong).unwrap();
                responder.publish(reply, payload.into()).await.unwrap();
            }
        }
    });

    let result: Result<Pong, NatsError> =
        request(&client, "test.msg.request", &Ping { value: 7 }).await;

    assert!(result.is_ok(), "request should succeed: {result:?}");
    assert_eq!(result.unwrap(), Pong { echoed: 7 });
}

/// `request_with_timeout()` returns `NatsError::Timeout` when no responder is present.
#[tokio::test]
async fn request_with_timeout_times_out_when_no_responder() {
    let (_container, port) = start_nats().await;
    let client = nats_client(port).await;

    let result: Result<Pong, NatsError> = request_with_timeout(
        &client,
        "test.msg.no_responder",
        &Ping { value: 1 },
        Duration::from_millis(200),
    )
    .await;

    assert!(
        matches!(result, Err(NatsError::Timeout { .. })),
        "expected Timeout error, got: {result:?}",
    );
}

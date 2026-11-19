#![allow(clippy::expect_used, clippy::unwrap_used)]

use async_nats::jetstream::stream::Config;
use trogon_nats::test_support::JetStreamTestServer;
use uuid::Uuid;

#[tokio::test]
async fn named_network_keeps_jetstream_servers_reachable_and_isolated() {
    let network = format!("jetstream-test-{}", Uuid::new_v4());
    let first = JetStreamTestServer::start_on_network(&network).await;
    let second = JetStreamTestServer::start_on_network(&network).await;
    assert_ne!(first.address(), second.address());

    let first_context = first.jetstream().await;
    let second_context = second.jetstream().await;
    let config = Config {
        name: "MESSAGES".to_owned(),
        subjects: vec!["messages".to_owned()],
        ..Default::default()
    };
    let mut first_stream = first_context.create_stream(config.clone()).await.unwrap();
    let mut second_stream = second_context.create_stream(config).await.unwrap();
    first_context
        .publish("messages", "first server only".into())
        .await
        .unwrap()
        .await
        .unwrap();

    assert_eq!(first_stream.info().await.unwrap().state.messages, 1);
    assert_eq!(second_stream.info().await.unwrap().state.messages, 0);
}

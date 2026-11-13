use super::*;
use crate::test_support::CoreTestServer;
use async_nats::client::SubscribeErrorKind;
use futures::StreamExt;
use std::time::Duration;

const SUBJECT: &str = "queue.subscribe.test";
const QUEUE_GROUP: &str = "workers";
const DELIVERY_DEADLINE: Duration = Duration::from_secs(5);
const DUPLICATE_WINDOW: Duration = Duration::from_millis(300);

async fn connect(server: &CoreTestServer) -> NatsAsyncClient {
    async_nats::connect(server.address()).await.unwrap()
}

#[tokio::test]
async fn queue_subscribe_refuses_a_queue_group_with_whitespace() {
    let server = CoreTestServer::start().await;
    let client = connect(&server).await;

    let error = QueueSubscribeClient::queue_subscribe(&client, SUBJECT, "two words".to_owned())
        .await
        .err()
        .unwrap();

    assert_eq!(error.kind(), SubscribeErrorKind::InvalidQueueName);
}

#[tokio::test]
async fn queue_subscribe_delivers_each_message_to_one_member_of_the_group() {
    let server = CoreTestServer::start().await;
    let client = connect(&server).await;
    let first = QueueSubscribeClient::queue_subscribe(&client, SUBJECT, QUEUE_GROUP.to_owned())
        .await
        .unwrap();
    let second = QueueSubscribeClient::queue_subscribe(&client, SUBJECT, QUEUE_GROUP.to_owned())
        .await
        .unwrap();
    let mut members = futures::stream::select(first, second);

    client.publish(SUBJECT, "once".into()).await.unwrap();
    client.flush().await.unwrap();

    let delivered = tokio::time::timeout(DELIVERY_DEADLINE, members.next())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(delivered.payload, "once");
    assert!(
        tokio::time::timeout(DUPLICATE_WINDOW, members.next()).await.is_err(),
        "a queue group delivers each message to one member only"
    );
}

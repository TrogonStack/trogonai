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

#[tokio::test]
async fn the_client_traits_publish_subscribe_flush_and_request_against_a_live_server() {
    let server = CoreTestServer::start().await;
    let client = connect(&server).await;

    // `async_nats::Client` has inherent methods with these names, so call through the traits.
    let mut subscription = SubscribeClient::subscribe(&client, "client.events").await.unwrap();
    let mut headers = HeaderMap::new();
    headers.insert("x-trace", "abc");
    PublishClient::publish_with_headers(&client, "client.events", headers, Bytes::from_static(b"hello"))
        .await
        .unwrap();
    FlushClient::flush(&client).await.unwrap();

    let received = tokio::time::timeout(DELIVERY_DEADLINE, subscription.next())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(received.payload, "hello");
    assert_eq!(received.headers.unwrap().get("x-trace").unwrap().as_str(), "abc");

    let mut requests = client.subscribe("client.echo").await.unwrap();
    let responder_client = client.clone();
    let responder = tokio::spawn(async move {
        let request = requests.next().await.expect("the request must arrive");
        responder_client
            .publish(
                request.reply.expect("a request carries a reply subject"),
                request.payload,
            )
            .await
            .expect("the reply must publish");
    });

    let reply = tokio::time::timeout(
        DELIVERY_DEADLINE,
        RequestClient::request_with_headers(&client, "client.echo", HeaderMap::new(), Bytes::from_static(b"ping")),
    )
    .await;
    // Check the responder first, so a failure there is reported instead of the request's timeout.
    tokio::time::timeout(DELIVERY_DEADLINE, responder)
        .await
        .expect("the responder must finish")
        .expect("the responder must not panic");
    assert_eq!(reply.expect("timeout").unwrap().payload, "ping");
}

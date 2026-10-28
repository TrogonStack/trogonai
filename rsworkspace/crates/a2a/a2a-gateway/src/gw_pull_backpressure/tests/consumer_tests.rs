use a2a_nats::constants::{GATEWAY_CALLER_ID_HEADER, GATEWAY_PRINCIPAL_HEADER, REQ_ID_HEADER};
use a2a_nats::nats::subjects::A2aStream;
use bytes::Bytes;
use trogon_nats::test_support::JetStreamTestServer;

use super::*;
use crate::gateway_test_support::Diagnostics;

const WAIT: Duration = Duration::from_secs(10);

fn config() -> GatewayEventsPullConfig {
    GatewayEventsPullConfig::new(
        GatewayEventsMaxAckPending::new(8),
        GatewayEventsFetchBatch::new(8),
        GatewayEventsFetchHeartbeat::new(Duration::from_secs(1)),
        GatewayEventsMaxInflightPerCaller::new(1),
    )
}

async fn next_message(subscriber: &mut async_nats::Subscriber) -> async_nats::Message {
    tokio::time::timeout(WAIT, subscriber.next()).await.unwrap().unwrap()
}

async fn wait_for_acknowledged(stream: &async_nats::jetstream::stream::Stream, expected_sequence: u64) {
    let durable = EventsConsumerDurable::for_prefix(&prefix());
    tokio::time::timeout(WAIT, async {
        loop {
            if let Ok(info) = stream.consumer_info(durable.as_str()).await
                && info.ack_floor.stream_sequence >= expected_sequence
                && info.num_ack_pending == 0
                && info.num_pending == 0
            {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("every routed or terminated event reaches its terminal acknowledgement");
}

#[tokio::test]
async fn pull_routes_by_caller_and_principal_and_terminates_unroutable_records() {
    let server = JetStreamTestServer::start().await;
    let client = server.client().await;
    let js = jetstream::new(client.clone());
    let stream = js.create_stream(A2aStream::Events.config(&prefix())).await.unwrap();
    let mut egress = client.subscribe("a2a.v1.gateway.>").await.unwrap();
    client.flush().await.unwrap();

    for (routing, payload) in [
        (
            headers(&[(GATEWAY_CALLER_ID_HEADER, "alice"), (REQ_ID_HEADER, "request-1")]),
            "event-1",
        ),
        (
            headers(&[(GATEWAY_CALLER_ID_HEADER, "alice"), (REQ_ID_HEADER, "request-2")]),
            "event-2",
        ),
        (
            headers(&[(GATEWAY_PRINCIPAL_HEADER, "bob"), (REQ_ID_HEADER, "request-3")]),
            "event-3",
        ),
        (headers(&[(REQ_ID_HEADER, "missing-caller")]), "unroutable-1"),
        (
            headers(&[(GATEWAY_CALLER_ID_HEADER, "*"), (REQ_ID_HEADER, "wildcard-caller")]),
            "unroutable-2",
        ),
        (
            headers(&[(GATEWAY_CALLER_ID_HEADER, "alice"), (REQ_ID_HEADER, " ")]),
            "unroutable-3",
        ),
    ] {
        js.publish_with_headers(
            "a2a.v1.tasks.task-1.events",
            routing,
            Bytes::from_static(payload.as_bytes()),
        )
        .await
        .unwrap()
        .await
        .unwrap();
    }

    let shutdown = CancellationToken::new();
    let worker = tokio::spawn(run_gateway_events_pull(client, prefix(), config(), shutdown.clone()));
    let mut delivered = Vec::new();
    for _ in 0..3 {
        let message = next_message(&mut egress).await;
        delivered.push((
            message.subject.to_string(),
            message
                .headers
                .as_ref()
                .unwrap()
                .get(REQ_ID_HEADER)
                .unwrap()
                .as_str()
                .to_owned(),
            String::from_utf8(message.payload.to_vec()).unwrap(),
        ));
    }
    delivered.sort();
    assert_eq!(
        delivered,
        vec![
            (
                "a2a.v1.gateway.alice.events".to_owned(),
                "request-1".to_owned(),
                "event-1".to_owned()
            ),
            (
                "a2a.v1.gateway.alice.events".to_owned(),
                "request-2".to_owned(),
                "event-2".to_owned()
            ),
            (
                "a2a.v1.gateway.bob.events".to_owned(),
                "request-3".to_owned(),
                "event-3".to_owned()
            ),
        ]
    );
    wait_for_acknowledged(&stream, 6).await;
    shutdown.cancel();
    tokio::time::timeout(Duration::from_secs(1), worker)
        .await
        .unwrap()
        .unwrap();
    assert!(
        tokio::time::timeout(Duration::from_millis(100), egress.next())
            .await
            .is_err()
    );
}

#[tokio::test]
async fn pull_recovers_when_the_events_stream_is_provisioned_after_startup() {
    let diagnostics = Diagnostics::both_outputs();
    let server = JetStreamTestServer::start().await;
    let client = server.client().await;
    let js = jetstream::new(client.clone());
    let mut attempts = client.subscribe("$JS.API.STREAM.INFO.A2A_EVENTS").await.unwrap();
    let mut egress = client.subscribe("a2a.v1.gateway.alice.events").await.unwrap();
    client.flush().await.unwrap();
    let shutdown = CancellationToken::new();
    let worker = tokio::spawn(run_gateway_events_pull(client, prefix(), config(), shutdown.clone()));

    next_message(&mut attempts).await;
    next_message(&mut attempts).await;
    let stream = js.create_stream(A2aStream::Events.config(&prefix())).await.unwrap();
    js.publish_with_headers(
        "a2a.v1.tasks.task-recovery.events",
        headers(&[
            (GATEWAY_CALLER_ID_HEADER, "alice"),
            (REQ_ID_HEADER, "recovered-request"),
        ]),
        Bytes::from_static(b"recovered-event"),
    )
    .await
    .unwrap()
    .await
    .unwrap();
    let event = next_message(&mut egress).await;
    assert_eq!(event.payload.as_ref(), b"recovered-event");
    assert_eq!(
        event.headers.as_ref().unwrap().get(REQ_ID_HEADER).unwrap().as_str(),
        "recovered-request"
    );
    wait_for_acknowledged(&stream, 1).await;
    shutdown.cancel();
    tokio::time::timeout(Duration::from_secs(1), worker)
        .await
        .unwrap()
        .unwrap();
    diagnostics.assert_event(
        "gateway events pull consumer started",
        &[
            ("durable", "A2A_GATEWAY_EVENTS"),
            ("max_ack_pending", "8"),
            ("fetch_batch", "8"),
            ("fetch_heartbeat_secs", "1"),
            ("max_inflight_per_caller", "1"),
        ],
    );
    diagnostics.assert_event(
        "gateway events pull cycle failed; backing off",
        &[("durable", "A2A_GATEWAY_EVENTS"), ("backoff_ms", "250")],
    );
}

#[tokio::test]
async fn pull_cancels_when_an_incompatible_durable_prevents_startup() {
    let server = JetStreamTestServer::start().await;
    let client = server.client().await;
    let js = jetstream::new(client.clone());
    let stream = js.create_stream(A2aStream::Events.config(&prefix())).await.unwrap();
    let durable = EventsConsumerDurable::for_prefix(&prefix());
    stream
        .create_consumer(async_nats::jetstream::consumer::push::Config {
            durable_name: Some(durable.as_str().to_owned()),
            deliver_subject: "_INBOX.existing-events-consumer".into(),
            ..Default::default()
        })
        .await
        .unwrap();
    let mut attempts = client
        .subscribe("$JS.API.CONSUMER.INFO.A2A_EVENTS.A2A_GATEWAY_EVENTS")
        .await
        .unwrap();
    client.flush().await.unwrap();
    let shutdown = CancellationToken::new();
    let worker = tokio::spawn(run_gateway_events_pull(client, prefix(), config(), shutdown.clone()));
    next_message(&mut attempts).await;
    next_message(&mut attempts).await;
    shutdown.cancel();
    tokio::time::timeout(Duration::from_secs(1), worker)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        stream
            .consumer_info(durable.as_str())
            .await
            .unwrap()
            .config
            .deliver_subject
            .as_deref(),
        Some("_INBOX.existing-events-consumer")
    );
}

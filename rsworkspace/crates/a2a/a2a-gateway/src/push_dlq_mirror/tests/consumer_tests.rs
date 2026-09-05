use std::time::Duration;

use async_nats::jetstream::stream::{DiscardPolicy, Stream};
use trogon_nats::test_support::JetStreamTestServer;

use super::*;

const WAIT: Duration = Duration::from_secs(10);

async fn wait_for_acknowledged(stream: &Stream, durable: &PushDlqMirrorDurable, expected_sequence: u64) {
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
    .expect("mirror acknowledges all source and mirror records");
}

#[tokio::test]
async fn consumer_mirrors_once_and_acknowledges_duplicate_invalid_and_loop_marked_records() {
    let server = JetStreamTestServer::start().await;
    let client = server.client().await;
    let js = async_nats::jetstream::new(client.clone());
    let mut stream = js.create_stream(A2aStream::PushDlq.config(&prefix())).await.unwrap();
    let mut mirrors = client.subscribe("a2a.v1.push.dlq.mirror.>").await.unwrap();
    client.flush().await.unwrap();

    let payload = Bytes::from_static(br#"{"idempotency_key":"task-1:failed"}"#);
    let mut headers = HeaderMap::new();
    headers.insert(NATS_MSG_ID_HEADER, "task-1:failed");
    headers.insert("Content-Type", "application/json");
    js.publish_with_headers("a2a.v1.push.dlq.alice.task-1", headers, payload.clone())
        .await
        .unwrap()
        .await
        .unwrap();
    js.publish("a2a.v1.push.dlq.alice.task-1", payload.clone())
        .await
        .unwrap()
        .await
        .unwrap();
    js.publish("a2a.v1.push.dlq.alice.task-2", Bytes::from_static(b"invalid-json"))
        .await
        .unwrap()
        .await
        .unwrap();
    let mut loop_headers = HeaderMap::new();
    loop_headers.insert(PUSH_DLQ_MIRROR_HEADER, "true");
    js.publish_with_headers(
        "a2a.v1.push.dlq.alice.task-3",
        loop_headers,
        Bytes::from_static(br#"{"idempotency_key":"task-3:failed"}"#),
    )
    .await
    .unwrap()
    .await
    .unwrap();

    let durable = PushDlqMirrorDurable::default_durable();
    let shutdown = CancellationToken::new();
    let worker = tokio::spawn(run_push_dlq_mirror(
        js,
        prefix(),
        durable.clone(),
        shutdown.clone(),
        Arc::new(PushDlqDedupGate::with_capacity(32)),
    ));
    let mirror = tokio::time::timeout(WAIT, mirrors.next()).await.unwrap().unwrap();
    assert_eq!(mirror.subject.as_str(), "a2a.v1.push.dlq.mirror.alice.task-1");
    assert_eq!(mirror.payload, payload);
    let mirror_headers = mirror.headers.as_ref().unwrap();
    assert_eq!(
        mirror_headers.get(NATS_MSG_ID_HEADER).unwrap().as_str(),
        "mirror:task-1:failed"
    );
    assert_eq!(mirror_headers.get(PUSH_DLQ_MIRROR_HEADER).unwrap().as_str(), "true");
    assert_eq!(mirror_headers.get("Content-Type").unwrap().as_str(), "application/json");

    wait_for_acknowledged(&stream, &durable, 5).await;
    shutdown.cancel();
    tokio::time::timeout(WAIT, worker).await.unwrap().unwrap();
    assert_eq!(stream.info().await.unwrap().state.messages, 5);
}

#[tokio::test]
async fn consumer_naks_full_stream_and_mirrors_redelivery_when_capacity_recovers() {
    let server = JetStreamTestServer::start().await;
    let client = server.client().await;
    let js = async_nats::jetstream::new(client.clone());
    let mut config = A2aStream::PushDlq.config(&prefix());
    config.max_messages = 1;
    config.discard = DiscardPolicy::New;
    let mut stream = js.create_stream(config.clone()).await.unwrap();
    let mut mirrors = client.subscribe("a2a.v1.push.dlq.mirror.>").await.unwrap();
    client.flush().await.unwrap();
    let payload = Bytes::from_static(br#"{"idempotency_key":"recover:failed"}"#);
    js.publish("a2a.v1.push.dlq.alice.recover", payload.clone())
        .await
        .unwrap()
        .await
        .unwrap();

    let durable = PushDlqMirrorDurable::default_durable();
    let shutdown = CancellationToken::new();
    let worker = tokio::spawn(run_push_dlq_mirror(
        js.clone(),
        prefix(),
        durable.clone(),
        shutdown.clone(),
        Arc::new(PushDlqDedupGate::with_capacity(32)),
    ));

    tokio::time::timeout(WAIT, async {
        loop {
            if let Ok(info) = stream.consumer_info(durable.as_str()).await
                && info.num_redelivered > 0
            {
                assert_eq!(info.ack_floor.stream_sequence, 0);
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("failed mirror publication leaves the source available for redelivery");

    config.max_messages = 10;
    js.update_stream(config).await.unwrap();
    let mirror = tokio::time::timeout(WAIT, mirrors.next()).await.unwrap().unwrap();
    assert_eq!(mirror.subject.as_str(), "a2a.v1.push.dlq.mirror.alice.recover");
    assert_eq!(mirror.payload, payload);
    wait_for_acknowledged(&stream, &durable, 2).await;
    shutdown.cancel();
    tokio::time::timeout(WAIT, worker).await.unwrap().unwrap();
    assert_eq!(stream.info().await.unwrap().state.messages, 2);
}

#[tokio::test]
async fn consumer_returns_when_its_authoritative_stream_is_missing() {
    let server = JetStreamTestServer::start().await;
    let js = server.jetstream().await;
    tokio::time::timeout(
        WAIT,
        run_push_dlq_mirror(
            js,
            prefix(),
            PushDlqMirrorDurable::default_durable(),
            CancellationToken::new(),
            Arc::new(PushDlqDedupGate::with_capacity(32)),
        ),
    )
    .await
    .expect("missing stream stops the mirror without hanging");
}

#[tokio::test]
async fn consumer_returns_when_its_durable_belongs_to_a_push_consumer() {
    let server = JetStreamTestServer::start().await;
    let js = server.jetstream().await;
    let stream = js.create_stream(A2aStream::PushDlq.config(&prefix())).await.unwrap();
    let durable = PushDlqMirrorDurable::default_durable();
    stream
        .create_consumer(async_nats::jetstream::consumer::push::Config {
            durable_name: Some(durable.as_str().to_owned()),
            deliver_subject: "_INBOX.existing-mirror-consumer".into(),
            ..Default::default()
        })
        .await
        .unwrap();

    tokio::time::timeout(
        WAIT,
        run_push_dlq_mirror(
            js,
            prefix(),
            durable.clone(),
            CancellationToken::new(),
            Arc::new(PushDlqDedupGate::with_capacity(32)),
        ),
    )
    .await
    .expect("an incompatible durable stops the mirror without hanging");
    assert_eq!(
        stream
            .consumer_info(durable.as_str())
            .await
            .unwrap()
            .config
            .deliver_subject
            .as_deref(),
        Some("_INBOX.existing-mirror-consumer")
    );
}

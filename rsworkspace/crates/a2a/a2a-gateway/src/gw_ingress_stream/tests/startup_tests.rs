use std::time::Duration;

use a2a_nats::nats::subjects::subscriptions::TaskAllEventsSubject;
use a2a_nats::nats::subjects::tasks::TaskEventsSubject;
use async_nats::jetstream::{consumer, stream};
use serde_json::json;
use trogon_nats::test_support::JetStreamTestServer;

use super::super::*;

fn config() -> GatewayStreamingIngressConfig {
    GatewayStreamingIngressConfig::new(StreamingMaxAckPending::new(1), StreamingMaxInflightPerCaller::new(1))
}

fn spawn(caller: &CallerKey) -> StreamingIngressSpawn {
    StreamingIngressSpawn {
        kind: StreamingIngressKind::TasksResubscribe {
            req_id: ReqId::from_header("resume-request"),
            task_id: A2aTaskId::new("task-1").expect("task"),
            last_seq: 0,
        },
        reply: "_INBOX.startup".into(),
        caller_key: caller.clone(),
    }
}

#[tokio::test]
async fn event_head_tracks_persisted_events_and_missing_storage_is_not_a_cursor() {
    let server = JetStreamTestServer::start().await;
    let client = server.client().await;
    let js = jetstream::new(client.clone());
    let prefix = A2aPrefix::new("startup").expect("prefix");
    assert_eq!(events_stream_last_seq(&client, &prefix).await, None);
    let name = events_stream_name(&prefix);
    js.create_stream(stream::Config {
        name: name.clone(),
        subjects: vec![TaskAllEventsSubject::new(&prefix).to_string()],
        ..Default::default()
    })
    .await
    .expect("events stream");
    assert_eq!(events_stream_last_seq(&client, &prefix).await, Some(0));
    let subject = TaskEventsSubject::new(&prefix, &A2aTaskId::new("task-1").expect("task")).to_string();
    let ack = js
        .publish(subject, bytes::Bytes::from_static(b"stored event"))
        .await
        .expect("publish")
        .await
        .expect("stored");
    assert_eq!(events_stream_last_seq(&client, &prefix).await, Some(ack.sequence));
    js.delete_stream(name).await.expect("remove events stream");
    assert_eq!(events_stream_last_seq(&client, &prefix).await, None);
}

#[tokio::test]
async fn missing_stream_returns_the_callers_admission_permit() {
    let server = JetStreamTestServer::start().await;
    let client = server.client().await;
    let prefix = A2aPrefix::new("startup").expect("prefix");
    let caller = CallerKey::new("alice").expect("caller");
    let gate = StreamingIngressGate::new(config());
    let permit = gate.try_acquire(&caller).expect("first request");
    assert!(gate.try_acquire(&caller).is_none());
    tokio::time::timeout(
        Duration::from_secs(5),
        run_streaming_ingress_pump(
            client,
            prefix,
            config(),
            permit,
            spawn(&caller),
            CancellationToken::new(),
        ),
    )
    .await
    .expect("failed startup returns");
    assert!(gate.try_acquire(&caller).is_some());
}

#[tokio::test]
async fn exhausted_broker_consumer_capacity_returns_the_callers_admission_permit() {
    let server = JetStreamTestServer::start().await;
    let client = server.client().await;
    let js = jetstream::new(client.clone());
    let prefix = A2aPrefix::new("startup").expect("prefix");
    let mut stream = js
        .create_stream(stream::Config {
            name: events_stream_name(&prefix),
            subjects: vec![TaskAllEventsSubject::new(&prefix).to_string()],
            max_consumers: 1,
            ..Default::default()
        })
        .await
        .expect("events stream");
    stream
        .create_consumer(consumer::pull::Config {
            durable_name: Some("reserved".into()),
            ..Default::default()
        })
        .await
        .expect("reserve sole consumer slot");
    let caller = CallerKey::new("alice").expect("caller");
    let gate = StreamingIngressGate::new(config());
    let permit = gate.try_acquire(&caller).expect("first request");
    tokio::time::timeout(
        Duration::from_secs(5),
        run_streaming_ingress_pump(
            client,
            prefix,
            config(),
            permit,
            spawn(&caller),
            CancellationToken::new(),
        ),
    )
    .await
    .expect("rejected consumer returns");
    assert!(gate.try_acquire(&caller).is_some());
    assert_eq!(
        stream
            .info()
            .await
            .expect("stream remains available")
            .state
            .consumer_count,
        1
    );
}

#[test]
fn unsupported_metadata_cursors_restart_at_zero_without_changing_task_identity() {
    for cursor in [
        json!(true),
        json!({"sequence": 9}),
        json!([9]),
        json!(null),
        json!(-1),
        json!(1.5),
    ] {
        let (task, sequence) = parse_resubscribe_params(&json!({
            "id": "task-1", "metadata": {"lastEventId": cursor}
        }))
        .expect("non-cursor metadata does not reject task");
        assert_eq!(task.as_str(), "task-1");
        assert_eq!(sequence, 0);
    }
}

use std::time::Duration;

use a2a_nats::constants::REQ_ID_HEADER;
use a2a_nats::jetstream::streams::events_stream_name;
use a2a_nats::nats::subjects::subscriptions::TaskAllEventsSubject;
use a2a_nats::nats::subjects::tasks::TaskEventsSubject;
use a2a_nats::{A2aTaskId, ReqId};
use async_nats::HeaderMap;
use async_nats::jetstream::{self, stream};
use bytes::Bytes;
use serde_json::json;
use tokio_util::sync::CancellationToken;

use crate::gw_ingress_stream::{
    CallerKey, StreamingIngressKind, StreamingIngressSpawn, StreamingIngressSpawnError, spawn_streaming_ingress_pump,
};

use super::fixture::{DispatchFixture, TestResult, assert_empty, receive, request};

#[tokio::test]
async fn message_stream_anchors_before_forward_and_delivers_only_matching_new_events() -> TestResult {
    let mut fixture = DispatchFixture::streaming().await?;
    let jetstream = jetstream::new(fixture.client.clone());
    jetstream
        .create_stream(stream::Config {
            name: events_stream_name(&fixture.config.a2a_prefix),
            subjects: vec![TaskAllEventsSubject::new(&fixture.config.a2a_prefix).to_string()],
            ..Default::default()
        })
        .await?;
    let event_subject = TaskEventsSubject::new(&fixture.config.a2a_prefix, &A2aTaskId::new("task-1")?).to_string();
    let mut matching_headers = HeaderMap::new();
    matching_headers.insert(REQ_ID_HEADER, "request-7");
    jetstream
        .publish_with_headers(
            event_subject.clone(),
            matching_headers.clone(),
            Bytes::from_static(b"historical"),
        )
        .await?
        .await?;

    fixture.dispatch(request("message.stream", json!({}))).await;
    assert_eq!(
        receive(&mut fixture.agents).await?.subject.as_str(),
        "a2a.v1.agents.bot.message.stream"
    );
    let audit = fixture.audit("ok").await?;
    assert_eq!(audit["stream_consumer"], "gateway.bot.message.stream");
    let mut other_headers = HeaderMap::new();
    other_headers.insert(REQ_ID_HEADER, "another-request");
    jetstream
        .publish_with_headers(
            event_subject.clone(),
            other_headers,
            Bytes::from_static(b"other-caller"),
        )
        .await?
        .await?;
    jetstream
        .publish_with_headers(event_subject, matching_headers, Bytes::from_static(b"new-event"))
        .await?
        .await?;
    assert_eq!(receive(&mut fixture.replies).await?.payload.as_ref(), b"new-event");
    fixture.shutdown.cancel();
    Ok(())
}

#[tokio::test]
async fn resubscribe_uses_task_cursor_and_shutdown_releases_caller_capacity() -> TestResult {
    let mut fixture = DispatchFixture::streaming().await?;
    let jetstream = jetstream::new(fixture.client.clone());
    jetstream
        .create_stream(stream::Config {
            name: events_stream_name(&fixture.config.a2a_prefix),
            subjects: vec![TaskAllEventsSubject::new(&fixture.config.a2a_prefix).to_string()],
            ..Default::default()
        })
        .await?;
    let task_id = A2aTaskId::new("task-1")?;
    let event_subject = TaskEventsSubject::new(&fixture.config.a2a_prefix, &task_id).to_string();
    let acknowledged = jetstream
        .publish(event_subject.clone(), Bytes::from_static(b"already-seen"))
        .await?
        .await?;
    fixture
        .dispatch(request(
            "tasks.resubscribe",
            json!({"id": "task-1", "last_seq": acknowledged.sequence}),
        ))
        .await;
    receive(&mut fixture.agents).await?;
    fixture.audit("ok").await?;
    let other_subject = TaskEventsSubject::new(&fixture.config.a2a_prefix, &A2aTaskId::new("other-task")?).to_string();
    jetstream
        .publish(other_subject, Bytes::from_static(b"other-task"))
        .await?
        .await?;
    jetstream
        .publish(event_subject.clone(), Bytes::from_static(b"resumed-event"))
        .await?
        .await?;
    assert_eq!(receive(&mut fixture.replies).await?.payload.as_ref(), b"resumed-event");

    let stopped = CancellationToken::new();
    stopped.cancel();
    let try_restart = || {
        spawn_streaming_ingress_pump(
            fixture.client.clone(),
            fixture.config.a2a_prefix.clone(),
            fixture.streaming_config,
            fixture.streaming_gate.clone(),
            StreamingIngressSpawn {
                kind: StreamingIngressKind::TasksResubscribe {
                    req_id: ReqId::from_header("restart"),
                    task_id: task_id.clone(),
                    last_seq: acknowledged.sequence,
                },
                reply: "_INBOX.dispatch".into(),
                caller_key: CallerKey::new("alice").expect("caller"),
            },
            stopped.clone(),
        )
    };
    assert!(matches!(
        try_restart(),
        Err(StreamingIngressSpawnError::PerCallerLimit { .. })
    ));
    fixture.shutdown.cancel();
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            match try_restart() {
                Ok(()) => break,
                Err(StreamingIngressSpawnError::PerCallerLimit { .. }) => tokio::task::yield_now().await,
            }
        }
    })
    .await?;
    jetstream
        .publish(event_subject, Bytes::from_static(b"after-shutdown"))
        .await?
        .await?;
    assert_empty(&fixture.client, &mut fixture.replies, "_INBOX.dispatch").await?;
    Ok(())
}

#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use std::time::Duration;

use async_nats::Request;
use async_nats::jetstream;
use trogon_decider_runtime::{CommandExecution, ReadFrom, ReadStreamRequest, StreamRead, TokioSnapshotTaskScheduler};
use trogon_nats::{NatsConfig, connect as nats_connect};
use trogon_scheduler::{
    CreateSchedule, GetScheduleCommand, PauseSchedule, RemoveSchedule, ResumeSchedule, ScheduleEventCase,
    ScheduleEventSchedule, ScheduleEventStatus, ScheduleId, commands::domain as command_domain, connect_store,
    get_schedule, state_v1, v1,
};

fn test_url() -> String {
    std::env::var("NATS_TEST_URL").unwrap_or_else(|_| "nats://localhost:4222".to_string())
}

fn command_schedule_id(id: &str) -> command_domain::ScheduleId {
    command_domain::ScheduleId::parse(id).unwrap()
}

async fn connect() -> async_nats::Client {
    let config = NatsConfig::from_url(test_url());
    nats_connect(&config, Duration::from_secs(10))
        .await
        .expect("failed to connect to NATS")
}

async fn connect_js() -> (async_nats::Client, jetstream::Context) {
    let nats = connect().await;
    let js = jetstream::new(nats.clone());
    (nats, js)
}

async fn reset_state(js: &jetstream::Context) {
    let _ = js.delete_stream(trogon_scheduler::kv::EVENTS_STREAM).await;
    if let Ok(kv) = js.get_key_value(trogon_scheduler::kv::SCHEDULES_BUCKET).await {
        let mut keys = kv.keys().await.unwrap();
        while let Some(result) = futures::StreamExt::next(&mut keys).await {
            let key = result.unwrap();
            let _ = kv.purge(key).await;
        }
    }
    if let Ok(kv) = js.get_key_value(trogon_scheduler::kv::COMMAND_SNAPSHOT_BUCKET).await {
        let mut keys = kv.keys().await.unwrap();
        while let Some(result) = futures::StreamExt::next(&mut keys).await {
            let key = result.unwrap();
            let _ = kv.purge(key).await;
        }
    }
}

fn base_schedule(id: &str) -> CreateSchedule {
    CreateSchedule {
        id: command_schedule_id(id),
        status: command_domain::ScheduleEventStatus::Scheduled,
        schedule: command_domain::Schedule::every(Duration::from_secs(2)).unwrap(),
        delivery: command_domain::Delivery::NatsEvent {
            route: command_domain::DeliveryRoute::new("agent.run").unwrap(),
            ttl: Some(command_domain::TtlDuration::from_secs(30).unwrap()),
            source: None,
        },
        message: command_domain::ScheduleMessage {
            content: command_domain::MessageContent::from_static(r#"{"kind":"heartbeat"}"#),
            headers: command_domain::ScheduleHeaders::default(),
        },
    }
}

#[tokio::test]
#[ignore = "requires nightly NATS scheduler"]
async fn raw_js_info_request_with_explicit_inbox_works() {
    let nats = connect().await;
    let inbox = nats.new_inbox();
    let response = nats
        .send_request(
            "$JS.API.INFO",
            Request::new()
                .inbox(inbox)
                .timeout(Some(Duration::from_secs(10)))
                .payload(br#"{}"#.as_slice().into()),
        )
        .await
        .unwrap();

    let body = String::from_utf8(response.payload.to_vec()).unwrap();
    assert!(body.contains("\"memory\""));
}

#[tokio::test]
#[ignore = "requires NATS test broker"]
async fn event_store_rebuilds_current_state_for_new_client() {
    let (nats, js) = connect_js().await;
    reset_state(&js).await;

    let store = connect_store(nats.clone()).await.unwrap();
    let mut job = base_schedule("eventful");
    job.schedule = command_domain::Schedule::cron("*/5 * * * * *", Some("UTC".to_string())).unwrap();
    let expected_schedule = ScheduleEventSchedule::Cron {
        expr: "*/5 * * * * *".to_string(),
        timezone: Some("UTC".to_string()),
    };

    CommandExecution::new(&store.event_store, &job).execute().await.unwrap();
    CommandExecution::new(&store.event_store, &PauseSchedule::new(command_schedule_id("eventful")))
        .with_snapshot(&store.event_store)
        .with_task_runtime(TokioSnapshotTaskScheduler)
        .execute()
        .await
        .unwrap();

    let fresh = connect_store(nats).await.unwrap();
    let rebuilt = get_schedule(
        &fresh.schedules_bucket,
        GetScheduleCommand::new(ScheduleId::parse("eventful").unwrap()),
    )
    .await
    .unwrap()
    .unwrap();

    assert_eq!(rebuilt.status, ScheduleEventStatus::Paused);
    assert_eq!(rebuilt.schedule, expected_schedule);
}

#[tokio::test]
#[ignore = "requires NATS test broker"]
async fn commands_execute_full_lifecycle_against_event_store() {
    let (nats, js) = connect_js().await;
    reset_state(&js).await;
    let store = connect_store(nats.clone()).await.unwrap();

    let job = base_schedule("lifecycle");
    let command_id = command_schedule_id("lifecycle");

    let added = CommandExecution::new(&store.event_store, &job).execute().await.unwrap();
    let added_position = added.stream_position;
    assert_eq!(
        added.state.state.as_ref().and_then(|value| value.as_known()),
        Some(state_v1::StateValue::STATE_VALUE_PRESENT_ENABLED)
    );

    let paused = CommandExecution::new(&store.event_store, &PauseSchedule::new(command_id.clone()))
        .with_snapshot(&store.event_store)
        .with_task_runtime(TokioSnapshotTaskScheduler)
        .execute()
        .await
        .unwrap();
    assert_eq!(paused.stream_position.as_u64(), added_position.as_u64() + 1);
    assert_eq!(
        paused.state.state.as_ref().and_then(|value| value.as_known()),
        Some(state_v1::StateValue::STATE_VALUE_PRESENT_DISABLED)
    );

    let resumed = CommandExecution::new(&store.event_store, &ResumeSchedule::new(command_id.clone()))
        .with_snapshot(&store.event_store)
        .with_task_runtime(TokioSnapshotTaskScheduler)
        .execute()
        .await
        .unwrap();
    assert_eq!(resumed.stream_position.as_u64(), paused.stream_position.as_u64() + 1);
    assert_eq!(
        resumed.state.state.as_ref().and_then(|value| value.as_known()),
        Some(state_v1::StateValue::STATE_VALUE_PRESENT_ENABLED)
    );

    let removed = CommandExecution::new(&store.event_store, &RemoveSchedule::new(command_id))
        .with_snapshot(&store.event_store)
        .with_task_runtime(TokioSnapshotTaskScheduler)
        .execute()
        .await
        .unwrap();
    assert_eq!(removed.stream_position.as_u64(), resumed.stream_position.as_u64() + 1);
    assert_eq!(
        removed.state.state.as_ref().and_then(|value| value.as_known()),
        Some(state_v1::StateValue::STATE_VALUE_DELETED)
    );

    let fresh = connect_store(nats).await.unwrap();
    let stream = fresh
        .event_store
        .read_stream(ReadStreamRequest {
            stream_id: "lifecycle",
            from: ReadFrom::Beginning,
        })
        .await
        .unwrap();
    assert_eq!(stream.current_position, Some(removed.stream_position));
    assert_eq!(stream.events.len(), 4);

    let events = stream
        .events
        .iter()
        .map(|event| event.decode::<v1::ScheduleEvent>().unwrap().into_decoded().unwrap())
        .collect::<Vec<_>>();
    assert!(matches!(&events[0].event, Some(ScheduleEventCase::ScheduleCreated(_))));
    assert!(matches!(&events[1].event, Some(ScheduleEventCase::SchedulePaused(_))));
    assert!(matches!(&events[2].event, Some(ScheduleEventCase::ScheduleResumed(_))));
    assert!(matches!(&events[3].event, Some(ScheduleEventCase::ScheduleRemoved(_))));
}

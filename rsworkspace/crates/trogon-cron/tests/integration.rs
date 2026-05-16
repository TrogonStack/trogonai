#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use std::time::Duration;

use async_nats::Request;
use async_nats::jetstream;
use chrono::{Duration as ChronoDuration, Utc};
use trogon_cron::{
    AddJobCommand, CronController, GetJobCommand, JobEventCase, JobEventSchedule, JobEventStatus, JobId,
    PauseJobCommand, RemoveJobCommand, ResumeJobCommand, commands::domain as command_domain, connect_store, get_job,
    state_v1, v1,
};
use trogon_eventsourcing::{CommandExecution, ReadStreamRequest, StreamRead, spawn_on_tokio};
use trogon_nats::{NatsConfig, connect as nats_connect};

fn test_url() -> String {
    std::env::var("NATS_TEST_URL").unwrap_or_else(|_| "nats://localhost:4222".to_string())
}

fn command_job_id(id: &str) -> command_domain::JobId {
    command_domain::JobId::parse(id).unwrap()
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
    let _ = trogon_cron::kv::get_or_create_schedule_stream(&js)
        .await
        .expect("failed to create schedule stream");
    (nats, js)
}

async fn reset_state(js: &jetstream::Context) {
    let _ = js.delete_stream(trogon_cron::kv::EVENTS_STREAM).await;
    if let Ok(stream) = js.get_stream(trogon_cron::kv::SCHEDULES_STREAM).await {
        let _ = stream.purge().await;
    }
    let _ = js.delete_stream("SENSORS").await;
    if let Ok(kv) = js.get_key_value(trogon_cron::kv::CRON_JOBS_BUCKET).await {
        let mut keys = kv.keys().await.unwrap();
        while let Some(result) = futures::StreamExt::next(&mut keys).await {
            let key = result.unwrap();
            let _ = kv.purge(key).await;
        }
    }
    if let Ok(kv) = js.get_key_value(trogon_cron::kv::COMMAND_SNAPSHOT_BUCKET).await {
        let mut keys = kv.keys().await.unwrap();
        while let Some(result) = futures::StreamExt::next(&mut keys).await {
            let key = result.unwrap();
            let _ = kv.purge(key).await;
        }
    }
    if let Ok(kv) = js.get_key_value(trogon_cron::kv::LEADER_BUCKET).await {
        let _ = kv.purge(trogon_cron::kv::LEADER_KEY).await;
    }
}

async fn wait_for_subject(stream: &jetstream::stream::Stream, subject: &str) {
    tokio::time::timeout(Duration::from_secs(12), async {
        loop {
            if stream.get_last_raw_message_by_subject(subject).await.is_ok() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
    })
    .await
    .expect("timed out waiting for scheduled subject");
}

async fn wait_for_subject_absence(stream: &jetstream::stream::Stream, subject: &str) {
    tokio::time::timeout(Duration::from_secs(12), async {
        loop {
            if stream.get_last_raw_message_by_subject(subject).await.is_err() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
    })
    .await
    .expect("timed out waiting for subject to disappear");
}

async fn wait_for_stream_subject(js: &jetstream::Context, stream_name: &str, subject: &str) {
    tokio::time::timeout(Duration::from_secs(12), async {
        loop {
            let stream = js.get_stream(stream_name).await.unwrap();
            if stream
                .cached_info()
                .config
                .subjects
                .iter()
                .any(|candidate| candidate == subject)
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
    })
    .await
    .expect("timed out waiting for stream subject to be configured");
}

fn base_job(id: &str) -> command_domain::Job {
    command_domain::Job {
        id: command_job_id(id),
        status: command_domain::JobStatus::Enabled,
        schedule: command_domain::Schedule::every(2).unwrap(),
        delivery: command_domain::Delivery::NatsEvent {
            route: command_domain::DeliveryRoute::new("agent.run").unwrap(),
            ttl_sec: Some(command_domain::TtlSeconds::new(30).unwrap()),
            source: None,
        },
        message: command_domain::JobMessage {
            content: command_domain::MessageContent::from_static(r#"{"kind":"heartbeat"}"#),
            headers: command_domain::JobHeaders::default(),
        },
    }
}

#[tokio::test]
#[ignore = "requires nightly NATS scheduler"]
async fn schedule_stream_has_required_flags() {
    let (_nats, js) = connect_js().await;
    reset_state(&js).await;

    let stream = js.get_stream(trogon_cron::kv::SCHEDULES_STREAM).await.unwrap();
    let info = stream.cached_info();

    assert!(info.config.allow_message_schedules);
    assert!(info.config.allow_message_ttl);
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
#[ignore = "requires nightly NATS scheduler"]
async fn controller_reconciles_one_time_job() {
    let (nats, js) = connect_js().await;
    reset_state(&js).await;
    let store = connect_store(nats.clone()).await.unwrap();

    let handle = tokio::spawn(async move {
        CronController::from_nats(nats).await.unwrap().run().await.unwrap();
    });

    let mut job = base_job("one-time");
    job.schedule = command_domain::Schedule::At {
        at: Utc::now() + ChronoDuration::seconds(2),
    };

    CommandExecution::new(&store.event_store, &AddJobCommand::new(job))
        .with_snapshot(&store.event_store)
        .with_task_runtime(spawn_on_tokio)
        .execute()
        .await
        .unwrap();

    let stream = js.get_stream(trogon_cron::kv::SCHEDULES_STREAM).await.unwrap();
    wait_for_subject(&stream, "cron.fire.agent.run.one-time").await;

    handle.abort();
}

#[tokio::test]
#[ignore = "requires nightly NATS scheduler"]
async fn controller_reconciles_sampling_job() {
    let (nats, js) = connect_js().await;
    reset_state(&js).await;
    let store = connect_store(nats.clone()).await.unwrap();

    let handle = tokio::spawn(async move {
        CronController::from_nats(nats).await.unwrap().run().await.unwrap();
    });

    let mut job = base_job("sampling");
    job.delivery = command_domain::Delivery::NatsEvent {
        route: command_domain::DeliveryRoute::new("agent.run").unwrap(),
        ttl_sec: None,
        source: Some(command_domain::SamplingSource::latest_from_subject("sensors.latest").unwrap()),
    };

    CommandExecution::new(&store.event_store, &AddJobCommand::new(job))
        .with_snapshot(&store.event_store)
        .with_task_runtime(spawn_on_tokio)
        .execute()
        .await
        .unwrap();
    wait_for_stream_subject(&js, trogon_cron::kv::SCHEDULES_STREAM, "sensors.latest").await;
    js.publish("sensors.latest", br#"{"value":42}"#.as_slice().into())
        .await
        .unwrap()
        .await
        .unwrap();

    let stream = js.get_stream(trogon_cron::kv::SCHEDULES_STREAM).await.unwrap();
    wait_for_subject(&stream, "cron.fire.agent.run.sampling").await;

    handle.abort();
}

#[tokio::test]
#[ignore = "requires nightly NATS scheduler"]
async fn controller_reconciles_cron_job_with_timezone() {
    let (nats, js) = connect_js().await;
    reset_state(&js).await;
    let store = connect_store(nats.clone()).await.unwrap();

    let handle = tokio::spawn(async move {
        CronController::from_nats(nats).await.unwrap().run().await.unwrap();
    });

    let mut job = base_job("cron-timezone");
    job.schedule = command_domain::Schedule::cron("*/2 * * * * *", Some("UTC".to_string())).unwrap();

    CommandExecution::new(&store.event_store, &AddJobCommand::new(job))
        .with_snapshot(&store.event_store)
        .with_task_runtime(spawn_on_tokio)
        .execute()
        .await
        .unwrap();

    let stream = js.get_stream(trogon_cron::kv::SCHEDULES_STREAM).await.unwrap();
    wait_for_subject(&stream, "cron.fire.agent.run.cron-timezone").await;

    handle.abort();
}

#[tokio::test]
#[ignore = "requires nightly NATS scheduler"]
async fn disabling_job_removes_schedule_subject() {
    let (nats, js) = connect_js().await;
    reset_state(&js).await;
    let store = connect_store(nats.clone()).await.unwrap();

    let handle = tokio::spawn(async move {
        CronController::from_nats(nats).await.unwrap().run().await.unwrap();
    });

    let job = base_job("disabled");
    CommandExecution::new(&store.event_store, &AddJobCommand::new(job))
        .with_snapshot(&store.event_store)
        .with_task_runtime(spawn_on_tokio)
        .execute()
        .await
        .unwrap();

    let stream = js.get_stream(trogon_cron::kv::SCHEDULES_STREAM).await.unwrap();
    wait_for_subject(&stream, "cron.schedules.disabled").await;

    CommandExecution::new(&store.event_store, &PauseJobCommand::new(command_job_id("disabled")))
        .with_snapshot(&store.event_store)
        .with_task_runtime(spawn_on_tokio)
        .execute()
        .await
        .unwrap();
    wait_for_subject_absence(&stream, "cron.schedules.disabled").await;

    handle.abort();
}

#[tokio::test]
#[ignore = "requires nightly NATS scheduler"]
async fn removing_job_removes_schedule_subject() {
    let (nats, js) = connect_js().await;
    reset_state(&js).await;
    let store = connect_store(nats.clone()).await.unwrap();

    let handle = tokio::spawn(async move {
        CronController::from_nats(nats).await.unwrap().run().await.unwrap();
    });

    let job = base_job("removed");
    CommandExecution::new(&store.event_store, &AddJobCommand::new(job))
        .with_snapshot(&store.event_store)
        .with_task_runtime(spawn_on_tokio)
        .execute()
        .await
        .unwrap();

    let stream = js.get_stream(trogon_cron::kv::SCHEDULES_STREAM).await.unwrap();
    wait_for_subject(&stream, "cron.schedules.removed").await;

    CommandExecution::new(&store.event_store, &RemoveJobCommand::new(command_job_id("removed")))
        .with_snapshot(&store.event_store)
        .with_task_runtime(spawn_on_tokio)
        .execute()
        .await
        .unwrap();
    wait_for_subject_absence(&stream, "cron.schedules.removed").await;

    handle.abort();
}

#[tokio::test]
#[ignore = "requires NATS test broker"]
async fn event_store_rebuilds_current_state_for_new_client() {
    let (nats, js) = connect_js().await;
    reset_state(&js).await;

    let store = connect_store(nats.clone()).await.unwrap();
    let mut job = base_job("eventful");
    job.schedule = command_domain::Schedule::cron("*/5 * * * * *", Some("UTC".to_string())).unwrap();
    let expected_schedule = JobEventSchedule::Cron {
        expr: "*/5 * * * * *".to_string(),
        timezone: Some("UTC".to_string()),
    };

    CommandExecution::new(&store.event_store, &AddJobCommand::new(job))
        .with_snapshot(&store.event_store)
        .with_task_runtime(spawn_on_tokio)
        .execute()
        .await
        .unwrap();
    CommandExecution::new(&store.event_store, &PauseJobCommand::new(command_job_id("eventful")))
        .with_snapshot(&store.event_store)
        .with_task_runtime(spawn_on_tokio)
        .execute()
        .await
        .unwrap();

    let fresh = connect_store(nats).await.unwrap();
    let rebuilt = get_job(
        &fresh.cron_jobs_bucket,
        GetJobCommand::new(JobId::parse("eventful").unwrap()),
    )
    .await
    .unwrap()
    .unwrap();

    assert_eq!(rebuilt.status, JobEventStatus::Disabled);
    assert_eq!(rebuilt.schedule, expected_schedule);
}

#[tokio::test]
#[ignore = "requires NATS test broker"]
async fn commands_execute_full_lifecycle_against_event_store() {
    let (nats, js) = connect_js().await;
    reset_state(&js).await;
    let store = connect_store(nats.clone()).await.unwrap();

    let job = base_job("lifecycle");
    let command_id = command_job_id("lifecycle");

    let added = CommandExecution::new(&store.event_store, &AddJobCommand::new(job))
        .with_snapshot(&store.event_store)
        .with_task_runtime(spawn_on_tokio)
        .execute()
        .await
        .unwrap();
    let added_position = added.stream_position;
    assert_eq!(
        added.state.state.as_ref().and_then(|value| value.as_known()),
        Some(state_v1::StateValue::STATE_VALUE_PRESENT_ENABLED)
    );

    let paused = CommandExecution::new(&store.event_store, &PauseJobCommand::new(command_id.clone()))
        .with_snapshot(&store.event_store)
        .with_task_runtime(spawn_on_tokio)
        .execute()
        .await
        .unwrap();
    assert_eq!(paused.stream_position.get(), added_position.get() + 1);
    assert_eq!(
        paused.state.state.as_ref().and_then(|value| value.as_known()),
        Some(state_v1::StateValue::STATE_VALUE_PRESENT_DISABLED)
    );

    let resumed = CommandExecution::new(&store.event_store, &ResumeJobCommand::new(command_id.clone()))
        .with_snapshot(&store.event_store)
        .with_task_runtime(spawn_on_tokio)
        .execute()
        .await
        .unwrap();
    assert_eq!(resumed.stream_position.get(), paused.stream_position.get() + 1);
    assert_eq!(
        resumed.state.state.as_ref().and_then(|value| value.as_known()),
        Some(state_v1::StateValue::STATE_VALUE_PRESENT_ENABLED)
    );

    let removed = CommandExecution::new(&store.event_store, &RemoveJobCommand::new(command_id))
        .with_snapshot(&store.event_store)
        .with_task_runtime(spawn_on_tokio)
        .execute()
        .await
        .unwrap();
    assert_eq!(removed.stream_position.get(), resumed.stream_position.get() + 1);
    assert_eq!(
        removed.state.state.as_ref().and_then(|value| value.as_known()),
        Some(state_v1::StateValue::STATE_VALUE_DELETED)
    );

    let fresh = connect_store(nats).await.unwrap();
    let stream = fresh
        .event_store
        .read_stream(ReadStreamRequest::new("lifecycle", 1))
        .await
        .unwrap();
    assert_eq!(stream.current_position, Some(removed.stream_position));
    assert_eq!(stream.events.len(), 4);

    let events = stream
        .events
        .iter()
        .map(|event| event.decode::<v1::JobEvent>().unwrap())
        .collect::<Vec<_>>();
    assert!(matches!(&events[0].event, Some(JobEventCase::JobAdded(_))));
    assert!(matches!(&events[1].event, Some(JobEventCase::JobPaused(_))));
    assert!(matches!(&events[2].event, Some(JobEventCase::JobResumed(_))));
    assert!(matches!(&events[3].event, Some(JobEventCase::JobRemoved(_))));
}

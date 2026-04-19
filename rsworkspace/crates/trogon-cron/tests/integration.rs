#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use std::time::Duration;

use async_nats::Request;
use async_nats::jetstream;
use chrono::{Duration as ChronoDuration, Utc};
use trogon_cron::{
    AddJobCommand, CronController, DeliveryRoute, DeliverySpec, GetJobCommand, JobEnabledState,
    JobEventState, JobId, JobSpec, MessageContent, MessageHeaders, PauseJobCommand,
    RemoveJobCommand, SamplingSource, ScheduleSpec, TtlSeconds, add_job, connect_store, get_job,
    pause_job, remove_job,
};
use trogon_nats::{NatsConfig, connect as nats_connect};

fn test_url() -> String {
    std::env::var("NATS_TEST_URL").unwrap_or_else(|_| "nats://localhost:4222".to_string())
}

fn job_id(id: &str) -> JobId {
    JobId::parse(id).unwrap()
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
    if let Ok(stream) = js.get_stream(trogon_cron::kv::EVENTS_STREAM).await {
        let _ = stream.purge().await;
    }
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
    if let Ok(kv) = js.get_key_value(trogon_cron::kv::SNAPSHOT_BUCKET).await {
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
            if stream
                .get_last_raw_message_by_subject(subject)
                .await
                .is_ok()
            {
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
            if stream
                .get_last_raw_message_by_subject(subject)
                .await
                .is_err()
            {
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

fn base_job(id: &str) -> JobSpec {
    JobSpec {
        id: job_id(id),
        state: JobEnabledState::Enabled,
        schedule: ScheduleSpec::Every { every_sec: 2 },
        delivery: DeliverySpec::NatsEvent {
            route: DeliveryRoute::new("agent.run").unwrap(),
            ttl_sec: Some(TtlSeconds::new(30).unwrap()),
            source: None,
        },
        content: MessageContent::from_static(br#"{"kind":"heartbeat"}"#),
        headers: MessageHeaders::default(),
    }
}

#[tokio::test]
#[ignore = "requires nightly NATS scheduler"]
async fn schedule_stream_has_required_flags() {
    let (_nats, js) = connect_js().await;
    reset_state(&js).await;

    let stream = js
        .get_stream(trogon_cron::kv::SCHEDULES_STREAM)
        .await
        .unwrap();
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
        CronController::from_nats(nats)
            .await
            .unwrap()
            .run()
            .await
            .unwrap();
    });

    let mut job = base_job("one-time");
    job.schedule = ScheduleSpec::At {
        at: Utc::now() + ChronoDuration::seconds(2),
    };

    add_job(
        store.command_runtime(),
        AddJobCommand::new(job).unwrap(),
        None,
    )
    .await
    .unwrap();

    let stream = js
        .get_stream(trogon_cron::kv::SCHEDULES_STREAM)
        .await
        .unwrap();
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
        CronController::from_nats(nats)
            .await
            .unwrap()
            .run()
            .await
            .unwrap();
    });

    let mut job = base_job("sampling");
    job.delivery = DeliverySpec::NatsEvent {
        route: DeliveryRoute::new("agent.run").unwrap(),
        ttl_sec: None,
        source: Some(SamplingSource::latest_from_subject("sensors.latest").unwrap()),
    };

    add_job(
        store.command_runtime(),
        AddJobCommand::new(job).unwrap(),
        None,
    )
    .await
    .unwrap();
    wait_for_stream_subject(&js, trogon_cron::kv::SCHEDULES_STREAM, "sensors.latest").await;
    js.publish("sensors.latest", br#"{"value":42}"#.as_slice().into())
        .await
        .unwrap()
        .await
        .unwrap();

    let stream = js
        .get_stream(trogon_cron::kv::SCHEDULES_STREAM)
        .await
        .unwrap();
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
        CronController::from_nats(nats)
            .await
            .unwrap()
            .run()
            .await
            .unwrap();
    });

    let mut job = base_job("cron-timezone");
    job.schedule = ScheduleSpec::Cron {
        expr: "*/2 * * * * *".to_string(),
        timezone: Some("UTC".to_string()),
    };

    add_job(
        store.command_runtime(),
        AddJobCommand::new(job).unwrap(),
        None,
    )
    .await
    .unwrap();

    let stream = js
        .get_stream(trogon_cron::kv::SCHEDULES_STREAM)
        .await
        .unwrap();
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
        CronController::from_nats(nats)
            .await
            .unwrap()
            .run()
            .await
            .unwrap();
    });

    let job = base_job("disabled");
    add_job(
        store.command_runtime(),
        AddJobCommand::new(job).unwrap(),
        None,
    )
    .await
    .unwrap();

    let stream = js
        .get_stream(trogon_cron::kv::SCHEDULES_STREAM)
        .await
        .unwrap();
    wait_for_subject(&stream, "cron.schedules.disabled").await;

    pause_job(
        store.command_runtime(),
        PauseJobCommand::new(job_id("disabled")),
        None,
    )
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
        CronController::from_nats(nats)
            .await
            .unwrap()
            .run()
            .await
            .unwrap();
    });

    let job = base_job("removed");
    add_job(
        store.command_runtime(),
        AddJobCommand::new(job).unwrap(),
        None,
    )
    .await
    .unwrap();

    let stream = js
        .get_stream(trogon_cron::kv::SCHEDULES_STREAM)
        .await
        .unwrap();
    wait_for_subject(&stream, "cron.schedules.removed").await;

    remove_job(
        store.command_runtime(),
        RemoveJobCommand::new(job_id("removed")),
        None,
    )
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
    job.schedule = ScheduleSpec::Cron {
        expr: "*/5 * * * * *".to_string(),
        timezone: Some("UTC".to_string()),
    };
    let expected_schedule = job.schedule.clone();

    add_job(
        store.command_runtime(),
        AddJobCommand::new(job).unwrap(),
        None,
    )
    .await
    .unwrap();
    pause_job(
        store.command_runtime(),
        PauseJobCommand::new(job_id("eventful")),
        None,
    )
    .await
    .unwrap();

    let fresh = connect_store(nats).await.unwrap();
    let rebuilt = get_job(
        &fresh,
        GetJobCommand {
            id: "eventful".to_string(),
        },
    )
    .await
    .unwrap()
    .unwrap();

    assert_eq!(rebuilt.state, JobEventState::Disabled);
    assert_eq!(rebuilt.schedule, expected_schedule.into());
}

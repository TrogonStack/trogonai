#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use std::time::Duration;

use async_nats::Request;
use async_nats::jetstream;
use chrono::{DateTime, Utc};
use tracing::instrument::WithSubscriber;
use trogon_decider_runtime::{
    CommandExecution, ReadFrom, ReadSnapshotRequest, ReadSnapshotResponse, ReadStreamRequest, Snapshot, SnapshotRead,
    SnapshotWrite, StreamRead, TokioSnapshotTaskScheduler, WriteSnapshotRequest,
};
use trogon_scheduler::{
    CreateSchedule, GetScheduleCommand, ListSchedulesCommand, PauseSchedule, RecordScheduleOccurrence, RemoveSchedule,
    ResumeSchedule, ScheduleEventCase, ScheduleEventSchedule, ScheduleEventStatus, ScheduleId, ScheduleNextOccurrence,
    commands::domain as command_domain, connect_store, get_schedule, list_schedules, state_v1, v1,
};
use trogon_std::log_capture::{CapturedLogs, LogLevel};

#[path = "support/events.rs"]
mod events;
#[path = "support/nats.rs"]
mod nats_support;

#[path = "support/projection_observer.rs"]
mod projection_observer;

fn fixture_schedule_id(label: &str) -> String {
    match label {
        "eventful" => "00000000000000000000000000000001",
        "retired" => "00000000000000000000000000000002",
        "recurring" => "00000000000000000000000000000003",
        "second" => "00000000000000000000000000000004",
        "report-v2" => "00000000000000000000000000000005",
        "orders-created" => "00000000000000000000000000000006",
        "namespace-thing" => "00000000000000000000000000000007",
        "nightly" => "00000000000000000000000000000008",
        "finite" => "00000000000000000000000000000009",
        "alpha" => "0000000000000000000000000000000a",
        "durable" => "0000000000000000000000000000000b",
        "lifecycle" => "0000000000000000000000000000000c",
        _ => panic!("missing explicit schedule ID fixture for {label}"),
    }
    .to_string()
}

fn command_schedule_id(id: &str) -> command_domain::ScheduleId {
    command_domain::ScheduleId::parse(&fixture_schedule_id(id)).unwrap()
}

fn query_schedule_id(id: &str) -> ScheduleId {
    ScheduleId::parse(&fixture_schedule_id(id)).unwrap()
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
async fn catch_up_skips_invalid_events_without_losing_valid_state_or_checkpoint_progress() {
    let (_server, client) = nats_support::start().await;
    let store = connect_store(client.clone()).await.unwrap();
    let js = jetstream::new(client.clone());
    CommandExecution::new(&store.event_store, &base_schedule("eventful"))
        .execute()
        .await
        .unwrap();
    events::publish_anomalies(&js, &fixture_schedule_id("retired"), &fixture_schedule_id("eventful")).await;
    js.publish(
        format!(
            "{}{}",
            trogon_scheduler::constants::EVENTS_SUBJECT_PREFIX,
            fixture_schedule_id("retired")
        ),
        bytes::Bytes::from_static(b"missing event headers"),
    )
    .await
    .unwrap()
    .await
    .unwrap();
    CommandExecution::new(&store.event_store, &PauseSchedule::new(command_schedule_id("eventful")))
        .execute()
        .await
        .unwrap();

    let fresh = tokio::time::timeout(Duration::from_secs(10), connect_store(client))
        .await
        .expect("invalid events must not wedge catch-up")
        .unwrap();
    let schedules = list_schedules(&fresh.schedules_bucket, ListSchedulesCommand)
        .await
        .unwrap();
    assert_eq!(schedules.len(), 1);
    assert_eq!(schedules[0].id, fixture_schedule_id("eventful"));
    assert_eq!(schedules[0].status, ScheduleEventStatus::Paused);
    let target = js
        .get_stream(trogon_scheduler::constants::EVENTS_STREAM)
        .await
        .unwrap()
        .get_info()
        .await
        .unwrap()
        .state
        .last_sequence;
    let checkpoint = fresh
        .schedules_bucket
        .get(trogon_scheduler::SCHEDULES_CHECKPOINT_KEY)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(checkpoint.as_ref(), target.to_string().as_bytes());
}

#[tokio::test]
async fn provisioning_repairs_legacy_stream_settings_and_preserves_wider_deduplication() {
    let (_server, client) = nats_support::start().await;
    let js = jetstream::new(client);
    let stream = trogon_scheduler::kv::get_or_create_events_stream(&js).await.unwrap();
    let mut legacy = stream.cached_info().config.clone();
    legacy.allow_atomic_publish = false;
    legacy.subjects = vec!["legacy.schedule.events.>".to_string()];
    legacy.duplicate_window = Duration::from_secs(1);
    js.update_stream(legacy).await.unwrap();

    let repaired = trogon_scheduler::kv::get_or_create_events_stream(&js).await.unwrap();
    let config = &repaired.cached_info().config;
    assert!(config.allow_atomic_publish);
    assert_eq!(
        config.subjects,
        vec![trogon_scheduler::constants::EVENTS_SUBJECT_PATTERN]
    );
    assert_eq!(
        config.duplicate_window,
        trogon_scheduler::constants::EVENTS_DUPLICATE_WINDOW.as_duration()
    );

    let mut wider = config.clone();
    wider.duplicate_window *= 2;
    let expected_window = wider.duplicate_window;
    js.update_stream(wider).await.unwrap();
    let reopened = trogon_scheduler::kv::get_or_create_events_stream(&js).await.unwrap();
    assert_eq!(reopened.cached_info().config.duplicate_window, expected_window);
}

#[tokio::test]
async fn startup_rejects_event_retention_that_can_erase_schedule_history() {
    let (_server, client) = nats_support::start().await;
    let js = jetstream::new(client.clone());
    let stream = trogon_scheduler::kv::get_or_create_events_stream(&js).await.unwrap();
    let mut limited = stream.cached_info().config.clone();
    limited.max_messages = 10;
    js.update_stream(limited).await.unwrap();
    let error = connect_store(client)
        .await
        .err()
        .expect("lossy retention must prevent startup");
    assert!(matches!(error, trogon_scheduler::SchedulerError::Event { .. }));
    assert!(std::error::Error::source(&error).is_some());
    assert_eq!(
        js.get_stream(trogon_scheduler::constants::EVENTS_STREAM)
            .await
            .unwrap()
            .cached_info()
            .config
            .max_messages,
        10
    );
}

#[tokio::test]
async fn key_value_provisioning_reopens_an_existing_bucket_and_reports_invalid_names() {
    let (_server, client) = nats_support::start().await;
    let js = jetstream::new(client);
    let original = js
        .create_key_value(jetstream::kv::Config {
            bucket: "PROVISIONING".to_string(),
            history: 5,
            ..Default::default()
        })
        .await
        .unwrap();
    original
        .put("retained", bytes::Bytes::from_static(b"existing value"))
        .await
        .unwrap();
    let reopened = trogon_scheduler::kv::get_or_create(
        &js,
        jetstream::kv::Config {
            bucket: "PROVISIONING".to_string(),
            history: 1,
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(
        reopened.get("retained").await.unwrap().unwrap().as_ref(),
        b"existing value"
    );
    let error = trogon_scheduler::kv::get_or_create(
        &js,
        jetstream::kv::Config {
            bucket: "invalid.bucket".to_string(),
            ..Default::default()
        },
    )
    .await
    .expect_err("invalid bucket names must fail");
    assert!(matches!(error, trogon_scheduler::SchedulerError::Kv { .. }));
    assert!(std::error::Error::source(&error).is_some());
}

#[tokio::test]
async fn listing_skips_a_corrupt_row_and_restart_repairs_it_from_history() {
    let (_server, client) = nats_support::start().await;
    let store = connect_store(client.clone()).await.unwrap();
    for id in ["eventful", "second"] {
        CommandExecution::new(&store.event_store, &base_schedule(id))
            .execute()
            .await
            .unwrap();
    }
    store
        .schedules_bucket
        .put(
            fixture_schedule_id("eventful"),
            bytes::Bytes::from_static(b"invalid projection"),
        )
        .await
        .unwrap();
    let listed = list_schedules(&store.schedules_bucket, ListSchedulesCommand)
        .await
        .unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].id, fixture_schedule_id("second"));
    CommandExecution::new(&store.event_store, &PauseSchedule::new(command_schedule_id("eventful")))
        .execute()
        .await
        .unwrap();
    let fresh = connect_store(client).await.unwrap();
    let repaired = get_schedule(
        &fresh.schedules_bucket,
        GetScheduleCommand::new(query_schedule_id("eventful")),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(repaired.status, ScheduleEventStatus::Paused);
    assert_eq!(
        list_schedules(&fresh.schedules_bucket, ListSchedulesCommand)
            .await
            .unwrap()
            .len(),
        2
    );
}

#[tokio::test]
async fn failed_projection_writes_leave_checkpoint_behind_until_replay_can_repair_them() {
    let logs = CapturedLogs::isolated();
    let (_server, client) = nats_support::start().await;
    let store = connect_store(client.clone()).await.unwrap();
    let js = jetstream::new(client.clone());
    js.update_key_value(jetstream::kv::Config {
        bucket: trogon_scheduler::SCHEDULES_BUCKET.to_string(),
        history: 5,
        max_value_size: 32,
        ..Default::default()
    })
    .await
    .unwrap();
    let observer = projection_observer::ProjectionFailureObserver::default();
    let command = base_schedule("eventful");
    let execution = CommandExecution::new(&store.event_store, &command);
    let appended = if logs.is_some() {
        execution.execute().await
    } else {
        execution.execute().with_subscriber(observer.clone()).await
    }
    .expect("projection failure must not turn a durable append into a failed command");
    if let Some(logs) = logs {
        let records = logs.records();
        let position = format!("stream_position={}", appended.stream_position.as_u64());
        let schedule_id = format!("schedule_id={}", fixture_schedule_id("eventful"));
        assert!(records.iter().any(|record| {
            record.level == LogLevel::Error
                && record.target == "trogon_scheduler::store::event_store"
                && record
                    .message
                    .contains("failed to project appended schedule events into the read model")
                && record.message.split_whitespace().any(|field| field == position)
                && record.message.contains(&schedule_id)
        }));
    } else {
        assert_eq!(observer.position(), Some(appended.stream_position.as_u64()));
    }

    let incomplete = connect_store(client.clone()).await.unwrap();
    assert!(
        get_schedule(
            &incomplete.schedules_bucket,
            GetScheduleCommand::new(query_schedule_id("eventful"))
        )
        .await
        .unwrap()
        .is_none()
    );
    assert!(
        incomplete
            .schedules_bucket
            .get(trogon_scheduler::SCHEDULES_CHECKPOINT_KEY)
            .await
            .unwrap()
            .is_none()
    );
    let recorded = incomplete
        .event_store
        .read_stream(ReadStreamRequest {
            stream_id: &command_schedule_id("eventful"),
            from: ReadFrom::Beginning,
        })
        .await
        .unwrap();
    assert_eq!(recorded.events.len(), 1);

    js.update_key_value(jetstream::kv::Config {
        bucket: trogon_scheduler::SCHEDULES_BUCKET.to_string(),
        history: 5,
        ..Default::default()
    })
    .await
    .unwrap();
    let repaired = connect_store(client).await.unwrap();
    assert!(
        get_schedule(
            &repaired.schedules_bucket,
            GetScheduleCommand::new(query_schedule_id("eventful"))
        )
        .await
        .unwrap()
        .is_some()
    );
    assert_eq!(
        repaired
            .schedules_bucket
            .get(trogon_scheduler::SCHEDULES_CHECKPOINT_KEY)
            .await
            .unwrap()
            .unwrap()
            .as_ref(),
        b"1"
    );
}

#[tokio::test]
async fn raw_js_info_request_with_explicit_inbox_works() {
    let (_server, nats) = nats_support::start().await;
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
async fn event_store_rebuilds_current_state_for_new_client() {
    let (_server, nats) = nats_support::start().await;

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
        GetScheduleCommand::new(query_schedule_id("eventful")),
    )
    .await
    .unwrap()
    .unwrap();

    assert_eq!(rebuilt.status, ScheduleEventStatus::Paused);
    assert_eq!(rebuilt.schedule, expected_schedule);
}

#[tokio::test]
async fn removed_schedule_reads_back_as_absent() {
    let (_server, nats) = nats_support::start().await;
    let store = connect_store(nats.clone()).await.unwrap();

    let id = command_schedule_id("retired");
    CommandExecution::new(&store.event_store, &base_schedule("retired"))
        .execute()
        .await
        .unwrap();
    CommandExecution::new(&store.event_store, &RemoveSchedule::new(id))
        .with_snapshot(&store.event_store)
        .with_task_runtime(TokioSnapshotTaskScheduler)
        .execute()
        .await
        .unwrap();

    // The projection deletes the key on removal, leaving a KV tombstone. Both
    // the point read and the listing must treat that tombstone as absent rather
    // than failing to deserialize its empty value.
    let queried = query_schedule_id("retired");
    assert!(
        get_schedule(&store.schedules_bucket, GetScheduleCommand::new(queried.clone()))
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        list_schedules(&store.schedules_bucket, ListSchedulesCommand)
            .await
            .unwrap()
            .is_empty()
    );

    // A fresh client rebuilds the read model from the event stream and must
    // reach the same absent result through the catch-up path.
    let fresh = connect_store(nats).await.unwrap();
    assert!(
        get_schedule(&fresh.schedules_bucket, GetScheduleCommand::new(queried))
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn catch_up_rebuilds_read_model_after_a_multi_event_append() {
    let (_server, nats) = nats_support::start().await;
    let store = connect_store(nats.clone()).await.unwrap();

    // A recurring schedule recording an occurrence appends two events at once
    // (recorded + follow-up), which leaves the read-model checkpoint behind the
    // stream tail. Catch-up must still rebuild the read model on the next start.
    let id = command_schedule_id("recurring");
    let mut create = base_schedule("recurring");
    create.schedule = command_domain::Schedule::rrule("2026-06-03T00:00:00Z", "FREQ=DAILY;COUNT=5", None).unwrap();
    CommandExecution::new(&store.event_store, &create)
        .execute()
        .await
        .unwrap();

    let now = DateTime::parse_from_rfc3339("2026-06-04T00:00:00Z")
        .unwrap()
        .with_timezone(&Utc);
    CommandExecution::new(&store.event_store, &ScheduleNextOccurrence::new(id.clone(), now))
        .execute()
        .await
        .unwrap();

    let armed = store
        .event_store
        .read_stream(ReadStreamRequest {
            stream_id: &id,
            from: ReadFrom::Beginning,
        })
        .await
        .unwrap()
        .events
        .iter()
        .filter_map(|event| event.decode::<v1::ScheduleEvent>().unwrap().into_decoded())
        .find_map(|event| match event.event {
            Some(ScheduleEventCase::ScheduleOccurrenceScheduled(scheduled)) => Some(
                trogonai_proto::convert::datetime_from_timestamp(scheduled.occurrence_at.as_option().unwrap()).unwrap(),
            ),
            _ => None,
        })
        .expect("an occurrence was armed");

    CommandExecution::new(
        &store.event_store,
        &RecordScheduleOccurrence::new(id.clone(), armed, now),
    )
    .execute()
    .await
    .unwrap();

    // A second schedule created after the stall: its ScheduleCreated now lives in
    // the replay window while it is already present in the KV.
    CommandExecution::new(&store.event_store, &base_schedule("second"))
        .execute()
        .await
        .unwrap();

    // The fresh client must rebuild from the event stream without failing and
    // surface both schedules.
    let fresh = connect_store(nats).await.unwrap();
    assert!(
        get_schedule(
            &fresh.schedules_bucket,
            GetScheduleCommand::new(query_schedule_id("recurring"))
        )
        .await
        .unwrap()
        .is_some()
    );
    assert!(
        get_schedule(
            &fresh.schedules_bucket,
            GetScheduleCommand::new(query_schedule_id("second"))
        )
        .await
        .unwrap()
        .is_some()
    );
}

async fn purge_schedules_bucket(js: &jetstream::Context) {
    let kv = js.get_key_value(trogon_scheduler::SCHEDULES_BUCKET).await.unwrap();
    let mut keys = kv.keys().await.unwrap();
    while let Some(result) = futures::StreamExt::next(&mut keys).await {
        let _ = kv.purge(result.unwrap()).await;
    }
}

#[tokio::test]
async fn projection_preserves_canonical_schedule_ids_through_live_and_catch_up() {
    let (_server, nats) = nats_support::start().await;
    let js = jetstream::new(nats.clone());
    let store = connect_store(nats.clone()).await.unwrap();

    let labels = ["report-v2", "orders-created", "namespace-thing", "nightly"];
    for id in labels {
        CommandExecution::new(&store.event_store, &base_schedule(id))
            .execute()
            .await
            .unwrap();
        assert!(
            get_schedule(&store.schedules_bucket, GetScheduleCommand::new(query_schedule_id(id)))
                .await
                .unwrap()
                .is_some(),
            "get could not address {id}"
        );
    }

    let live: Vec<String> = list_schedules(&store.schedules_bucket, ListSchedulesCommand)
        .await
        .unwrap()
        .into_iter()
        .map(|schedule| schedule.id)
        .collect();
    assert_eq!(live.len(), labels.len(), "unexpected live listing: {live:?}");
    for label in labels {
        let id = fixture_schedule_id(label);
        assert!(live.contains(&id), "live projection missing {label}");
    }

    // Drop the KV read model so a fresh client must re-fold the events.
    purge_schedules_bucket(&js).await;
    let fresh = connect_store(nats).await.unwrap();
    let rebuilt: Vec<String> = list_schedules(&fresh.schedules_bucket, ListSchedulesCommand)
        .await
        .unwrap()
        .into_iter()
        .map(|schedule| schedule.id)
        .collect();
    assert_eq!(rebuilt.len(), labels.len(), "unexpected rebuilt listing: {rebuilt:?}");
    for label in labels {
        let id = fixture_schedule_id(label);
        assert!(rebuilt.contains(&id), "catch-up rebuild missing {label}");
        assert!(
            get_schedule(
                &fresh.schedules_bucket,
                GetScheduleCommand::new(query_schedule_id(label))
            )
            .await
            .unwrap()
            .is_some(),
            "get could not address {label} after rebuild"
        );
    }
}

#[tokio::test]
async fn completed_recurring_schedule_is_marked_completed_in_read_model() {
    let (_server, nats) = nats_support::start().await;
    let js = jetstream::new(nats.clone());
    let store = connect_store(nats.clone()).await.unwrap();

    // A single-occurrence recurrence whose only occurrence is already in the past
    // exhausts the moment it is armed: arming emits ScheduleCompleted.
    let id = command_schedule_id("finite");
    let mut create = base_schedule("finite");
    create.schedule = command_domain::Schedule::rrule("2020-01-01T00:00:00Z", "FREQ=DAILY;COUNT=1", None).unwrap();
    CommandExecution::new(&store.event_store, &create)
        .execute()
        .await
        .unwrap();

    let now = DateTime::parse_from_rfc3339("2026-06-19T00:00:00Z")
        .unwrap()
        .with_timezone(&Utc);
    CommandExecution::new(&store.event_store, &ScheduleNextOccurrence::new(id.clone(), now))
        .execute()
        .await
        .unwrap();

    let completed_event = store
        .event_store
        .read_stream(ReadStreamRequest {
            stream_id: &id,
            from: ReadFrom::Beginning,
        })
        .await
        .unwrap()
        .events
        .iter()
        .filter_map(|event| event.decode::<v1::ScheduleEvent>().unwrap().into_decoded())
        .any(|event| matches!(event.event, Some(ScheduleEventCase::ScheduleCompleted(_))));
    assert!(
        completed_event,
        "arming an exhausted recurrence emits ScheduleCompleted"
    );

    let live = get_schedule(
        &store.schedules_bucket,
        GetScheduleCommand::new(query_schedule_id("finite")),
    )
    .await
    .unwrap()
    .expect("schedule still present after completion");
    assert!(live.completed, "completed recurring schedule must be marked completed");
    assert!(!live.is_enabled(), "a completed schedule must not be enabled");

    // The completion survives a catch-up rebuild.
    purge_schedules_bucket(&js).await;
    let fresh = connect_store(nats).await.unwrap();
    let rebuilt = get_schedule(
        &fresh.schedules_bucket,
        GetScheduleCommand::new(query_schedule_id("finite")),
    )
    .await
    .unwrap()
    .expect("schedule present after rebuild");
    assert!(rebuilt.completed, "completion must survive a catch-up rebuild");
}

#[tokio::test]
async fn catch_up_reconcile_removes_rows_absent_from_the_folded_state() {
    let (_server, nats) = nats_support::start().await;
    let js = jetstream::new(nats.clone());
    let store = connect_store(nats.clone()).await.unwrap();
    CommandExecution::new(&store.event_store, &base_schedule("alpha"))
        .execute()
        .await
        .unwrap();

    let kv = js.get_key_value(trogon_scheduler::SCHEDULES_BUCKET).await.unwrap();
    // Reuse a real projected value so the injected entries deserialize cleanly.
    let value = {
        let mut keys = kv.keys().await.unwrap();
        let mut found = None;
        while let Some(result) = futures::StreamExt::next(&mut keys).await {
            let key = result.unwrap();
            if key != trogon_scheduler::SCHEDULES_CHECKPOINT_KEY {
                found = kv.get(&key).await.unwrap();
                break;
            }
        }
        found.expect("alpha was projected")
    };
    // A pre-v2 raw-id key and an unrelated derived-format key — neither belongs to
    // the freshly folded state. Because catch-up replays the full event log from
    // empty, that folded state is authoritative: the reconcile deletes every row it
    // does not account for (relying on the single-active-writer invariant), so both
    // injected rows are removed while the genuinely folded schedule survives.
    let legacy_key = "legacy.raw.id".to_string();
    let orphan_derived_key = "0123456789abcdef0123456789abcdef".to_string();
    kv.put(legacy_key.clone(), value.clone()).await.unwrap();
    kv.put(orphan_derived_key.clone(), value.clone()).await.unwrap();
    // Force a rebuild (and therefore the reconcile) on the next start.
    let _ = kv.purge(trogon_scheduler::SCHEDULES_CHECKPOINT_KEY.to_string()).await;

    let fresh = connect_store(nats).await.unwrap();

    assert!(
        kv.get(&legacy_key).await.unwrap().is_none(),
        "a row absent from the folded state must be reconciled away"
    );
    assert!(
        kv.get(&orphan_derived_key).await.unwrap().is_none(),
        "a row absent from the folded state must be reconciled away"
    );
    assert!(
        get_schedule(
            &fresh.schedules_bucket,
            GetScheduleCommand::new(query_schedule_id("alpha"))
        )
        .await
        .unwrap()
        .is_some(),
        "the genuinely folded schedule survives the reconcile"
    );
}

#[tokio::test]
async fn catch_up_self_heals_from_a_corrupt_checkpoint() {
    let (_server, nats) = nats_support::start().await;
    let js = jetstream::new(nats.clone());
    let store = connect_store(nats.clone()).await.unwrap();
    CommandExecution::new(&store.event_store, &base_schedule("durable"))
        .execute()
        .await
        .unwrap();

    purge_schedules_bucket(&js).await;
    // Corrupt the checkpoint value: a non-numeric checkpoint must not wedge startup.
    let kv = js.get_key_value(trogon_scheduler::SCHEDULES_BUCKET).await.unwrap();
    kv.put(
        trogon_scheduler::SCHEDULES_CHECKPOINT_KEY.to_string(),
        "not-a-number".into(),
    )
    .await
    .unwrap();

    // A fresh client treats the corrupt checkpoint as 0 and rebuilds.
    let fresh = connect_store(nats).await.unwrap();
    assert!(
        get_schedule(
            &fresh.schedules_bucket,
            GetScheduleCommand::new(query_schedule_id("durable"))
        )
        .await
        .unwrap()
        .is_some()
    );
}

#[tokio::test]
async fn commands_execute_full_lifecycle_against_event_store() {
    let (_server, nats) = nats_support::start().await;
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

    let removed = CommandExecution::new(&store.event_store, &RemoveSchedule::new(command_id.clone()))
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
            stream_id: &command_id,
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

#[tokio::test]
async fn persisted_command_snapshot_round_trips_and_survives_reopening() {
    let (_server, client) = nats_support::start().await;
    let store = connect_store(client.clone()).await.unwrap();
    let id = command_schedule_id("lifecycle");
    let result = CommandExecution::new(&store.event_store, &base_schedule("lifecycle"))
        .execute()
        .await
        .unwrap();
    let info = store.event_store.events_stream().get_info().await.unwrap();
    assert_eq!(info.state.last_sequence, result.stream_position.as_u64());
    store
        .event_store
        .write_snapshot(WriteSnapshotRequest {
            snapshot_id: &id,
            snapshot: Snapshot::new(result.stream_position, result.state.clone()),
        })
        .await
        .unwrap();

    let js = jetstream::new(client.clone());
    let bucket = trogon_scheduler::open_command_snapshot_bucket(&js).await.unwrap();
    assert_eq!(
        bucket.status().await.unwrap().info.config.name,
        format!("KV_{}", trogon_scheduler::constants::COMMAND_SNAPSHOT_BUCKET)
    );
    let reopened = connect_store(client).await.unwrap();
    let snapshot: ReadSnapshotResponse<state_v1::State> = reopened
        .event_store
        .read_snapshot(ReadSnapshotRequest { snapshot_id: &id })
        .await
        .unwrap();
    let snapshot = snapshot.snapshot.unwrap();
    assert_eq!(snapshot.position, result.stream_position);
    assert_eq!(snapshot.payload, result.state);

    js.delete_key_value(trogon_scheduler::constants::COMMAND_SNAPSHOT_BUCKET)
        .await
        .unwrap();
    assert!(matches!(
        trogon_scheduler::open_command_snapshot_bucket(&js).await.err().unwrap(),
        trogon_scheduler::SchedulerError::Kv { .. }
    ));
}

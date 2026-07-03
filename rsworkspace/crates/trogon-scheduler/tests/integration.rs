#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]
// These tests require a live NATS broker (all `#[ignore]`d). The coverage build
// stubs out the NATS-backed store, so exclude the whole suite there.
#![cfg(not(coverage))]

use std::time::Duration;

use async_nats::Request;
use async_nats::jetstream;
use chrono::{DateTime, Utc};
use trogon_decider_runtime::{CommandExecution, ReadFrom, ReadStreamRequest, StreamRead, TokioSnapshotTaskScheduler};
use trogon_nats::{NatsConfig, connect as nats_connect};
use trogon_scheduler::{
    CreateSchedule, GetScheduleCommand, ListSchedulesCommand, PauseSchedule, RecordScheduleOccurrence, RemoveSchedule,
    ResumeSchedule, ScheduleEventCase, ScheduleEventSchedule, ScheduleEventStatus, ScheduleId, ScheduleNextOccurrence,
    commands::domain as command_domain, connect_store, get_schedule, list_schedules, state_v1, v1,
};
use trogon_std::env::{ReadEnv, SystemEnv};

fn test_url() -> String {
    SystemEnv
        .var("NATS_TEST_URL")
        .unwrap_or_else(|_| "nats://localhost:4222".to_string())
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
    if let Ok(kv) = js.get_key_value(trogon_scheduler::SCHEDULES_BUCKET).await {
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
async fn removed_schedule_reads_back_as_absent() {
    let (nats, js) = connect_js().await;
    reset_state(&js).await;
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
    let queried = ScheduleId::parse("retired").unwrap();
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
#[ignore = "requires NATS test broker"]
async fn catch_up_rebuilds_read_model_after_a_multi_event_append() {
    let (nats, js) = connect_js().await;
    reset_state(&js).await;
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
            stream_id: "recurring",
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
            GetScheduleCommand::new(ScheduleId::parse("recurring").unwrap())
        )
        .await
        .unwrap()
        .is_some()
    );
    assert!(
        get_schedule(
            &fresh.schedules_bucket,
            GetScheduleCommand::new(ScheduleId::parse("second").unwrap())
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
#[ignore = "requires NATS test broker"]
async fn projection_folds_permissive_schedule_ids_through_live_and_catch_up() {
    let (nats, js) = connect_js().await;
    reset_state(&js).await;
    let store = connect_store(nats.clone()).await.unwrap();

    // IDs the command domain accepts but a single NATS token would reject —
    // including ':' and non-ASCII that are not valid raw KV keys. The read model
    // keys by a derived token, so all of them must be addressable by get, present
    // in list, and survive a catch-up rebuild.
    let ids = ["report.v2", "orders/created", "ns:thing", "café-nightly"];
    for id in ids {
        CommandExecution::new(&store.event_store, &base_schedule(id))
            .execute()
            .await
            .unwrap();
        assert!(
            get_schedule(
                &store.schedules_bucket,
                GetScheduleCommand::new(ScheduleId::parse(id).unwrap())
            )
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
    assert_eq!(live.len(), ids.len(), "unexpected live listing: {live:?}");
    for id in ids {
        assert!(live.contains(&id.to_string()), "live projection missing {id}");
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
    assert_eq!(rebuilt.len(), ids.len(), "unexpected rebuilt listing: {rebuilt:?}");
    for id in ids {
        assert!(rebuilt.contains(&id.to_string()), "catch-up rebuild missing {id}");
        assert!(
            get_schedule(
                &fresh.schedules_bucket,
                GetScheduleCommand::new(ScheduleId::parse(id).unwrap())
            )
            .await
            .unwrap()
            .is_some(),
            "get could not address {id} after rebuild"
        );
    }
}

#[tokio::test]
#[ignore = "requires NATS test broker"]
async fn completed_recurring_schedule_is_marked_completed_in_read_model() {
    let (nats, js) = connect_js().await;
    reset_state(&js).await;
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
            stream_id: "finite",
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
        GetScheduleCommand::new(ScheduleId::parse("finite").unwrap()),
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
        GetScheduleCommand::new(ScheduleId::parse("finite").unwrap()),
    )
    .await
    .unwrap()
    .expect("schedule present after rebuild");
    assert!(rebuilt.completed, "completion must survive a catch-up rebuild");
}

#[tokio::test]
#[ignore = "requires NATS test broker"]
async fn catch_up_reconcile_removes_rows_absent_from_the_folded_state() {
    let (nats, js) = connect_js().await;
    reset_state(&js).await;
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
            GetScheduleCommand::new(ScheduleId::parse("alpha").unwrap())
        )
        .await
        .unwrap()
        .is_some(),
        "the genuinely folded schedule survives the reconcile"
    );
}

#[tokio::test]
#[ignore = "requires NATS test broker"]
async fn catch_up_self_heals_from_a_corrupt_checkpoint() {
    let (nats, js) = connect_js().await;
    reset_state(&js).await;
    let store = connect_store(nats.clone()).await.unwrap();
    CommandExecution::new(&store.event_store, &base_schedule("durable"))
        .execute()
        .await
        .unwrap();

    // Corrupt the checkpoint value: a non-numeric checkpoint must not wedge startup.
    let kv = js.get_key_value(trogon_scheduler::SCHEDULES_BUCKET).await.unwrap();
    kv.put(
        trogon_scheduler::SCHEDULES_CHECKPOINT_KEY.to_string(),
        "not-a-number".into(),
    )
    .await
    .unwrap();
    purge_schedules_bucket(&js).await;

    // A fresh client treats the corrupt checkpoint as 0 and rebuilds.
    let fresh = connect_store(nats).await.unwrap();
    assert!(
        get_schedule(
            &fresh.schedules_bucket,
            GetScheduleCommand::new(ScheduleId::parse("durable").unwrap())
        )
        .await
        .unwrap()
        .is_some()
    );
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

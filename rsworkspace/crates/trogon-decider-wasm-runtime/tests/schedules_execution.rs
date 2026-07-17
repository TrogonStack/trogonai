//! End-to-end execution tests against the scheduler schedules WASM bundle.
#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

mod support;

use std::fs;
use std::path::Path;

use buffa::Message as _;
use buffa::MessageField;
use buffa::MessageName as _;
use support::{InMemoryEventStore, InMemorySnapshotStore};
use trogon_decider_runtime::{
    DiscardAndReplaySnapshotFailure, ImmediateSnapshotTaskScheduler, ReadFrom, StreamPosition, StreamWritePrecondition,
};
use trogon_decider_wasm_runtime::{
    OpaqueSnapshotPayload, WasmCommandError, WasmCommandExecution, WasmDeciderEngine, WasmDeciderModule,
    WasmEngineConfig, WasmSnapshotId,
};
use trogon_decider_wit::host::CommandEnvelope;
use trogonai_proto::content::v1alpha1 as content_v1alpha1;
use trogonai_proto::scheduler::schedules::{CREATE_SCHEDULE_TYPE_URL, PAUSE_SCHEDULE_TYPE_URL, v1};

fn schedules_wasm() -> Vec<u8> {
    let relative = "../../target/wasm32-unknown-unknown/release/trogon_schedules_decider.wasm";
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(relative);
    fs::read(&path).unwrap_or_else(|error| {
        panic!(
            "build trogon_schedules_decider.wasm for wasm32-unknown-unknown first (expected {}): {error}",
            path.display()
        )
    })
}

fn schedules_module() -> WasmDeciderModule {
    let engine = WasmDeciderEngine::new(WasmEngineConfig::default()).expect("engine builds");
    WasmDeciderModule::load(engine, &schedules_wasm()).expect("module loads")
}

fn create_command(id: &str) -> CommandEnvelope {
    CommandEnvelope {
        type_: CREATE_SCHEDULE_TYPE_URL.to_string(),
        payload: v1::CreateSchedule {
            schedule_id: id.to_string(),
            status: MessageField::some(v1::ScheduleStatus {
                kind: Some(v1::schedule_status::Scheduled {}.into()),
            }),
            schedule: MessageField::some(v1::Schedule {
                kind: Some(
                    v1::schedule::Every {
                        every: MessageField::some(buffa_types::google::protobuf::Duration {
                            seconds: 30,
                            nanos: 0,
                            ..buffa_types::google::protobuf::Duration::default()
                        }),
                    }
                    .into(),
                ),
            }),
            delivery: MessageField::some(v1::Delivery {
                kind: Some(
                    v1::delivery::NatsMessage {
                        subject: "agent.run".to_string(),
                        ttl: MessageField::none(),
                        source: MessageField::none(),
                    }
                    .into(),
                ),
            }),
            message: MessageField::some(v1::Message {
                content: MessageField::some(content_v1alpha1::Content {
                    content_type: "application/json".to_string(),
                    data: br#"{"kind":"heartbeat"}"#.to_vec(),
                }),
                headers: Vec::new(),
            }),
        }
        .encode_to_vec(),
    }
}

fn pause_command(id: &str) -> CommandEnvelope {
    CommandEnvelope {
        type_: PAUSE_SCHEDULE_TYPE_URL.to_string(),
        payload: v1::PauseSchedule {
            schedule_id: id.to_string(),
        }
        .encode_to_vec(),
    }
}

fn unknown_command() -> CommandEnvelope {
    CommandEnvelope {
        type_: "type.googleapis.com/trogonai.scheduler.schedules.v1.DoesNotExist".to_string(),
        payload: Vec::new(),
    }
}

fn position(value: u64) -> StreamPosition {
    StreamPosition::try_new(value).expect("test stream position must be non-zero")
}

#[tokio::test]
async fn create_takes_the_no_stream_fast_path() {
    let module = schedules_module();
    let event_store = InMemoryEventStore::default();

    let result = WasmCommandExecution::new(&module, &event_store, &create_command("backup"))
        .execute()
        .await
        .expect("create succeeds");

    assert_eq!(result.stream_position, position(1));
    assert_eq!(result.events.len(), 1);
    assert_eq!(result.events[0].type_, v1::ScheduleCreated::FULL_NAME);
    assert_eq!(event_store.read_stream_calls(), 0);
    assert_eq!(
        event_store.write_preconditions(),
        vec![StreamWritePrecondition::NoStream]
    );
    assert_eq!(
        event_store.stored_event_types("backup"),
        vec![v1::ScheduleCreated::FULL_NAME.to_string()]
    );
}

#[tokio::test]
async fn pause_replays_history_and_appends_at_observed_position() {
    let module = schedules_module();
    let event_store = InMemoryEventStore::default();

    WasmCommandExecution::new(&module, &event_store, &create_command("backup"))
        .execute()
        .await
        .expect("create succeeds");
    let result = WasmCommandExecution::new(&module, &event_store, &pause_command("backup"))
        .execute()
        .await
        .expect("pause succeeds");

    assert_eq!(result.stream_position, position(2));
    assert_eq!(result.events.len(), 1);
    assert_eq!(event_store.reads_from(), vec![ReadFrom::Beginning]);
    assert_eq!(
        event_store.write_preconditions(),
        vec![
            StreamWritePrecondition::NoStream,
            StreamWritePrecondition::At(position(1))
        ]
    );
}

#[tokio::test]
async fn pausing_a_missing_schedule_is_rejected() {
    let module = schedules_module();
    let event_store = InMemoryEventStore::default();

    let Err(error) = WasmCommandExecution::new(&module, &event_store, &pause_command("missing"))
        .execute()
        .await
    else {
        panic!("expected rejection");
    };

    assert!(matches!(error, WasmCommandError::Rejected(_)), "{error}");
    assert_eq!(event_store.write_preconditions(), Vec::new());
}

#[tokio::test]
async fn an_unknown_command_type_fails_at_stream_id_resolution() {
    let module = schedules_module();
    let event_store = InMemoryEventStore::default();

    let Err(error) = WasmCommandExecution::new(&module, &event_store, &unknown_command())
        .execute()
        .await
    else {
        panic!("expected stream id resolution failure");
    };

    let WasmCommandError::StreamId(detail) = error else {
        panic!("expected stream id error, got {error}");
    };
    assert_eq!(detail.code, "invalid-command");
    assert_eq!(event_store.read_stream_calls(), 0);
}

#[tokio::test]
async fn snapshot_round_trip_matches_full_replay() {
    let module = schedules_module();
    let event_store = InMemoryEventStore::default();
    let snapshot_store = InMemorySnapshotStore::default();
    let scheduler = ImmediateSnapshotTaskScheduler;

    WasmCommandExecution::new(&module, &event_store, &create_command("backup"))
        .with_snapshot_store(&snapshot_store, &scheduler)
        .execute()
        .await
        .expect("create succeeds");

    let snapshot_id = WasmSnapshotId::new(module.name(), module.version(), "backup");
    let snapshot = snapshot_store
        .get(snapshot_id.as_str())
        .expect("create must write a snapshot");
    assert_eq!(snapshot.position, position(1));

    let result = WasmCommandExecution::new(&module, &event_store, &pause_command("backup"))
        .with_snapshot_store(&snapshot_store, &scheduler)
        .execute()
        .await
        .expect("pause resumed from the snapshot must be accepted");

    assert_eq!(result.stream_position, position(2));
    let expected_resume = ReadFrom::after(position(1)).expect("resume position advances");
    assert_eq!(event_store.reads_from(), vec![expected_resume]);
}

#[tokio::test]
async fn a_snapshot_ahead_of_the_stream_is_rejected() {
    let module = schedules_module();
    let event_store = InMemoryEventStore::default();
    let snapshot_store = InMemorySnapshotStore::default();
    let scheduler = ImmediateSnapshotTaskScheduler;

    let snapshot_id = WasmSnapshotId::new(module.name(), module.version(), "backup");
    snapshot_store.insert(
        snapshot_id.as_str(),
        trogon_decider_runtime::Snapshot::new(position(5), OpaqueSnapshotPayload::new(Vec::new())),
    );

    let Err(error) = WasmCommandExecution::new(&module, &event_store, &pause_command("backup"))
        .with_snapshot_store(&snapshot_store, &scheduler)
        .execute()
        .await
    else {
        panic!("expected snapshot ahead of stream error");
    };

    assert!(matches!(error, WasmCommandError::SnapshotAheadOfStream(_)), "{error}");
}

#[tokio::test]
async fn a_snapshot_read_failure_is_rejected_by_default() {
    let module = schedules_module();
    let event_store = InMemoryEventStore::default();
    let snapshot_store = InMemorySnapshotStore::default();
    let scheduler = ImmediateSnapshotTaskScheduler;

    WasmCommandExecution::new(&module, &event_store, &create_command("backup"))
        .execute()
        .await
        .expect("create succeeds");
    snapshot_store.fail_reads();

    let Err(error) = WasmCommandExecution::new(&module, &event_store, &pause_command("backup"))
        .with_snapshot_store(&snapshot_store, &scheduler)
        .execute()
        .await
    else {
        panic!("expected snapshot read failure");
    };

    assert!(matches!(error, WasmCommandError::ReadSnapshot(_)), "{error}");
    assert_eq!(event_store.read_stream_calls(), 0);
}

#[tokio::test]
async fn discard_and_replay_recovers_from_a_snapshot_read_failure() {
    let module = schedules_module();
    let event_store = InMemoryEventStore::default();
    let snapshot_store = InMemorySnapshotStore::default();
    let scheduler = ImmediateSnapshotTaskScheduler;

    WasmCommandExecution::new(&module, &event_store, &create_command("backup"))
        .execute()
        .await
        .expect("create succeeds");
    snapshot_store.fail_reads();

    let result = WasmCommandExecution::new(&module, &event_store, &pause_command("backup"))
        .with_snapshot_store(&snapshot_store, &scheduler)
        .with_snapshot_failure_policy(DiscardAndReplaySnapshotFailure)
        .execute()
        .await
        .expect("discard-and-replay recovers from the unreadable snapshot");

    assert_eq!(result.stream_position, position(2));
    assert_eq!(event_store.reads_from(), vec![ReadFrom::Beginning]);

    let snapshot_id = WasmSnapshotId::new(module.name(), module.version(), "backup");
    let snapshot = snapshot_store
        .get(snapshot_id.as_str())
        .expect("a fresh snapshot replaces the unreadable one");
    assert_eq!(snapshot.position, position(2));
}

#[tokio::test]
async fn discard_and_replay_recovers_from_a_snapshot_ahead_of_stream() {
    let module = schedules_module();
    let event_store = InMemoryEventStore::default();
    let snapshot_store = InMemorySnapshotStore::default();
    let scheduler = ImmediateSnapshotTaskScheduler;

    WasmCommandExecution::new(&module, &event_store, &create_command("backup"))
        .execute()
        .await
        .expect("create succeeds");

    let snapshot_id = WasmSnapshotId::new(module.name(), module.version(), "backup");
    snapshot_store.insert(
        snapshot_id.as_str(),
        trogon_decider_runtime::Snapshot::new(position(5), OpaqueSnapshotPayload::new(Vec::new())),
    );

    let result = WasmCommandExecution::new(&module, &event_store, &pause_command("backup"))
        .with_snapshot_store(&snapshot_store, &scheduler)
        .with_snapshot_failure_policy(DiscardAndReplaySnapshotFailure)
        .execute()
        .await
        .expect("discard-and-replay recovers from the ahead-of-stream snapshot");

    assert_eq!(result.stream_position, position(2));
    let stale_resume = ReadFrom::after(position(5)).expect("resume position advances");
    assert_eq!(event_store.reads_from(), vec![stale_resume, ReadFrom::Beginning]);

    let refreshed = snapshot_store
        .get(snapshot_id.as_str())
        .expect("a fresh snapshot replaces the ahead-of-stream one");
    assert_eq!(refreshed.position, position(2));
}

#[tokio::test]
async fn builder_overrides_shape_the_appended_events() {
    #[derive(Debug, Clone, Copy)]
    struct FixedUuidGenerator(uuid::Uuid);

    impl trogon_std::NowV7 for FixedUuidGenerator {
        fn now_v7(&self) -> uuid::Uuid {
            self.0
        }
    }

    let module = schedules_module();
    let event_store = InMemoryEventStore::default();
    let fixed_id = uuid::Uuid::now_v7();
    let header_name = trogon_decider_runtime::HeaderName::new("trace-id").expect("valid header name");
    let headers = trogon_decider_runtime::Headers::one(header_name, "abc-123").expect("valid header value");

    WasmCommandExecution::new(&module, &event_store, &create_command("backup"))
        .with_headers(headers)
        .with_event_id_generator(FixedUuidGenerator(fixed_id))
        .execute()
        .await
        .expect("create succeeds");

    let stored = event_store.stored_events("backup");
    assert_eq!(stored.len(), 1);
    assert_eq!(stored[0].id, trogon_decider_runtime::EventId::new(fixed_id));
    assert_eq!(stored[0].headers.get_str("trace-id"), Some("abc-123"));

    let result = WasmCommandExecution::new(&module, &event_store, &pause_command("backup"))
        .with_write_precondition(StreamWritePrecondition::Any)
        .execute()
        .await
        .expect("pause succeeds");

    assert_eq!(result.stream_position, position(2));
    assert_eq!(
        event_store.write_preconditions(),
        vec![StreamWritePrecondition::NoStream, StreamWritePrecondition::Any]
    );
}

#[tokio::test]
async fn an_empty_snapshot_store_falls_back_to_full_replay() {
    let module = schedules_module();
    let event_store = InMemoryEventStore::default();
    let snapshot_store = InMemorySnapshotStore::default();
    let scheduler = ImmediateSnapshotTaskScheduler;

    WasmCommandExecution::new(&module, &event_store, &create_command("backup"))
        .execute()
        .await
        .expect("create succeeds");

    let result = WasmCommandExecution::new(&module, &event_store, &pause_command("backup"))
        .with_snapshot_store(&snapshot_store, &scheduler)
        .execute()
        .await
        .expect("pause succeeds without a prior snapshot");

    assert_eq!(result.stream_position, position(2));
    assert_eq!(event_store.reads_from(), vec![ReadFrom::Beginning]);
    let snapshot_id = WasmSnapshotId::new(module.name(), module.version(), "backup");
    assert!(snapshot_store.get(snapshot_id.as_str()).is_some());
}

#[tokio::test]
async fn a_failing_snapshot_write_does_not_fail_the_command() {
    let module = schedules_module();
    let event_store = InMemoryEventStore::default();
    let snapshot_store = InMemorySnapshotStore::default();
    let scheduler = ImmediateSnapshotTaskScheduler;
    snapshot_store.fail_writes();

    let result = WasmCommandExecution::new(&module, &event_store, &create_command("backup"))
        .with_snapshot_store(&snapshot_store, &scheduler)
        .execute()
        .await
        .expect("create succeeds even when the snapshot write fails");

    assert_eq!(result.stream_position, position(1));
    let snapshot_id = WasmSnapshotId::new(module.name(), module.version(), "backup");
    assert!(snapshot_store.get(snapshot_id.as_str()).is_none());
}

#[test]
fn an_exhausted_fuel_budget_fails_the_load_probe() {
    let engine = WasmDeciderEngine::new(WasmEngineConfig::default().with_fuel_per_call(1)).expect("engine builds");
    assert!(WasmDeciderModule::load(engine, &schedules_wasm()).is_err());
}

#[tokio::test]
async fn the_pooling_allocator_engine_executes_the_fixture_end_to_end() {
    let engine = WasmDeciderEngine::new(
        WasmEngineConfig::default()
            .with_max_concurrent_sessions(2)
            .with_max_instances_per_session(3)
            .with_max_tables_per_session(2)
            .with_max_memories_per_session(1),
    )
    .expect("pooling-allocator engine builds");
    let module = WasmDeciderModule::load(engine, &schedules_wasm()).expect("module loads under the pooling allocator");
    let event_store = InMemoryEventStore::default();

    let create_result = WasmCommandExecution::new(&module, &event_store, &create_command("backup"))
        .execute()
        .await
        .expect("create succeeds under the pooling allocator");
    assert_eq!(create_result.stream_position, position(1));

    let pause_result = WasmCommandExecution::new(&module, &event_store, &pause_command("backup"))
        .execute()
        .await
        .expect("pause succeeds under the pooling allocator");
    assert_eq!(pause_result.stream_position, position(2));
}

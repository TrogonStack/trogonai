//! End-to-end execution tests against the scheduler schedules WASM bundle.
#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

mod support;

use std::fs;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};

use buffa::Message as _;
use buffa::MessageField;
use buffa::MessageName as _;
use support::{ContendedEventStore, InMemoryEventStore, InMemorySnapshotStore};
use trogon_decider_runtime::{
    AdmissionLimit, AuthorizationDeniedError, CommandAdmission, CommandAuthorizer, CommandPrincipal,
    ConcurrencyAdmission, ConflictRetryLimit, DiscardAndReplaySnapshotFailure, ImmediateSnapshotTaskScheduler,
    PreconditionConflictError, PrincipalClaim, PrincipalId, PrincipalKind, ReadFrom, ReplayChunkSize, ReplayLimit,
    SnapshotCadence, StreamPosition, StreamWritePrecondition, UnauthorizedError,
};
use trogon_decider_wasm_runtime::{
    OpaqueSnapshotPayload, WasmCommandError, WasmCommandExecution, WasmDeciderEngine, WasmDeciderModule,
    WasmEngineConfig, WasmSnapshotId,
};
use trogon_decider_wit::host::CommandEnvelope;
use trogonai_proto::content::v1alpha1 as content_v1alpha1;
use trogonai_proto::scheduler::schedules::{
    CREATE_SCHEDULE_TYPE_URL, PAUSE_SCHEDULE_TYPE_URL, RESUME_SCHEDULE_TYPE_URL, v1,
};

const SCHEDULE_ID: &str = "0198be07a38479e1a376f250f9181be9";
const MISSING_SCHEDULE_ID: &str = "0198be07a38479e1a376f250f9181bea";

fn schedules_wasm() -> Vec<u8> {
    let relative = "../../../target/wasm32-unknown-unknown/release/trogon_schedules_decider.wasm";
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

fn resume_command(id: &str) -> CommandEnvelope {
    CommandEnvelope {
        type_: RESUME_SCHEDULE_TYPE_URL.to_string(),
        payload: v1::ResumeSchedule {
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

fn replay_limit(value: u64) -> ReplayLimit {
    ReplayLimit::try_new(value).expect("test replay limit must be non-zero")
}

fn chunk_size(value: u64) -> ReplayChunkSize {
    ReplayChunkSize::try_new(value).expect("test chunk size must be non-zero")
}

#[tokio::test]
async fn create_takes_the_no_stream_fast_path() {
    let module = schedules_module();
    let event_store = InMemoryEventStore::default();

    let result = WasmCommandExecution::new(&module, &event_store, &create_command(SCHEDULE_ID))
        .execute()
        .await
        .expect("create succeeds");

    assert_eq!(result.stream_position, position(1));
    assert_eq!(result.events.len(), 1);
    assert_eq!(result.events[0].r#type, v1::ScheduleCreated::FULL_NAME);
    assert_eq!(event_store.read_stream_calls(), 0);
    assert_eq!(
        event_store.write_preconditions(),
        vec![StreamWritePrecondition::NoStream]
    );
    assert_eq!(
        event_store.stored_event_types(SCHEDULE_ID),
        vec![v1::ScheduleCreated::FULL_NAME.to_string()]
    );
}

#[tokio::test]
async fn a_descriptor_no_stream_precondition_skips_the_replay_read() {
    let module = schedules_module();
    let event_store = InMemoryEventStore::default();

    WasmCommandExecution::new(&module, &event_store, &create_command(SCHEDULE_ID))
        .execute()
        .await
        .expect("create succeeds");

    assert_eq!(event_store.read_stream_calls(), 0);
    assert_eq!(
        event_store.write_preconditions(),
        vec![StreamWritePrecondition::NoStream]
    );
}

#[tokio::test]
async fn an_expected_revision_conflicts_with_a_descriptor_no_stream_precondition() {
    let module = schedules_module();
    let event_store = InMemoryEventStore::default();

    let Err(error) = WasmCommandExecution::new(&module, &event_store, &create_command(SCHEDULE_ID))
        .with_expected_revision(position(1))
        .execute()
        .await
    else {
        panic!("expected a precondition conflict");
    };

    assert!(
        matches!(
            error,
            WasmCommandError::PreconditionConflict(PreconditionConflictError::CreateWithRevision)
        ),
        "{error}"
    );
    assert_eq!(event_store.read_stream_calls(), 0);
    assert!(event_store.write_preconditions().is_empty());
}

#[tokio::test]
async fn an_expected_revision_conflicting_with_a_create_skips_snapshot_and_stream_reads() {
    let module = schedules_module();
    let event_store = InMemoryEventStore::default();
    let snapshot_store = InMemorySnapshotStore::default();
    let scheduler = ImmediateSnapshotTaskScheduler;

    let Err(error) = WasmCommandExecution::new(&module, &event_store, &create_command(SCHEDULE_ID))
        .with_snapshot_store(&snapshot_store, &scheduler)
        .with_expected_revision(position(1))
        .execute()
        .await
    else {
        panic!("expected a precondition conflict");
    };

    assert!(
        matches!(
            error,
            WasmCommandError::PreconditionConflict(PreconditionConflictError::CreateWithRevision)
        ),
        "{error}"
    );
    assert_eq!(snapshot_store.read_snapshot_calls(), 0);
    assert_eq!(event_store.read_stream_calls(), 0);
}

#[tokio::test]
async fn pause_replays_history_and_appends_at_observed_position() {
    let module = schedules_module();
    let event_store = InMemoryEventStore::default();

    WasmCommandExecution::new(&module, &event_store, &create_command(SCHEDULE_ID))
        .execute()
        .await
        .expect("create succeeds");
    let result = WasmCommandExecution::new(&module, &event_store, &pause_command(SCHEDULE_ID))
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

    let Err(error) = WasmCommandExecution::new(&module, &event_store, &pause_command(MISSING_SCHEDULE_ID))
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

    WasmCommandExecution::new(&module, &event_store, &create_command(SCHEDULE_ID))
        .with_snapshot_store(&snapshot_store, &scheduler)
        .with_snapshot_cadence(SnapshotCadence::every_events(1))
        .execute()
        .await
        .expect("create succeeds");

    let snapshot_id = WasmSnapshotId::new(module.name(), module.version(), SCHEDULE_ID);
    let snapshot = snapshot_store
        .get(snapshot_id.as_str())
        .expect("create must write a snapshot");
    assert_eq!(snapshot.position, position(1));

    let result = WasmCommandExecution::new(&module, &event_store, &pause_command(SCHEDULE_ID))
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

    let snapshot_id = WasmSnapshotId::new(module.name(), module.version(), SCHEDULE_ID);
    snapshot_store.insert(
        snapshot_id.as_str(),
        trogon_decider_runtime::Snapshot::new(position(5), OpaqueSnapshotPayload::new(Vec::new())),
    );

    let Err(error) = WasmCommandExecution::new(&module, &event_store, &pause_command(SCHEDULE_ID))
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

    WasmCommandExecution::new(&module, &event_store, &create_command(SCHEDULE_ID))
        .execute()
        .await
        .expect("create succeeds");
    snapshot_store.fail_reads();

    let Err(error) = WasmCommandExecution::new(&module, &event_store, &pause_command(SCHEDULE_ID))
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

    WasmCommandExecution::new(&module, &event_store, &create_command(SCHEDULE_ID))
        .execute()
        .await
        .expect("create succeeds");
    snapshot_store.fail_reads();

    let result = WasmCommandExecution::new(&module, &event_store, &pause_command(SCHEDULE_ID))
        .with_snapshot_store(&snapshot_store, &scheduler)
        .with_snapshot_cadence(SnapshotCadence::every_events(1))
        .with_snapshot_failure_policy(DiscardAndReplaySnapshotFailure)
        .execute()
        .await
        .expect("discard-and-replay recovers from the unreadable snapshot");

    assert_eq!(result.stream_position, position(2));
    assert_eq!(event_store.reads_from(), vec![ReadFrom::Beginning]);

    let snapshot_id = WasmSnapshotId::new(module.name(), module.version(), SCHEDULE_ID);
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

    WasmCommandExecution::new(&module, &event_store, &create_command(SCHEDULE_ID))
        .execute()
        .await
        .expect("create succeeds");

    let snapshot_id = WasmSnapshotId::new(module.name(), module.version(), SCHEDULE_ID);
    snapshot_store.insert(
        snapshot_id.as_str(),
        trogon_decider_runtime::Snapshot::new(position(5), OpaqueSnapshotPayload::new(Vec::new())),
    );

    let result = WasmCommandExecution::new(&module, &event_store, &pause_command(SCHEDULE_ID))
        .with_snapshot_store(&snapshot_store, &scheduler)
        .with_snapshot_cadence(SnapshotCadence::every_events(1))
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

    WasmCommandExecution::new(&module, &event_store, &create_command(SCHEDULE_ID))
        .with_headers(headers)
        .with_event_id_generator(FixedUuidGenerator(fixed_id))
        .execute()
        .await
        .expect("create succeeds");

    let stored = event_store.stored_events(SCHEDULE_ID);
    assert_eq!(stored.len(), 1);
    assert_eq!(stored[0].id, trogon_decider_runtime::EventId::new(fixed_id));
    assert_eq!(stored[0].headers.get_str("trace-id"), Some("abc-123"));

    let result = WasmCommandExecution::new(&module, &event_store, &pause_command(SCHEDULE_ID))
        .execute()
        .await
        .expect("pause succeeds");

    assert_eq!(result.stream_position, position(2));
    assert_eq!(
        event_store.write_preconditions(),
        vec![
            StreamWritePrecondition::NoStream,
            StreamWritePrecondition::At(position(1))
        ]
    );
}

#[tokio::test]
async fn an_empty_snapshot_store_falls_back_to_full_replay() {
    let module = schedules_module();
    let event_store = InMemoryEventStore::default();
    let snapshot_store = InMemorySnapshotStore::default();
    let scheduler = ImmediateSnapshotTaskScheduler;

    WasmCommandExecution::new(&module, &event_store, &create_command(SCHEDULE_ID))
        .execute()
        .await
        .expect("create succeeds");

    let result = WasmCommandExecution::new(&module, &event_store, &pause_command(SCHEDULE_ID))
        .with_snapshot_store(&snapshot_store, &scheduler)
        .with_snapshot_cadence(SnapshotCadence::every_events(1))
        .execute()
        .await
        .expect("pause succeeds without a prior snapshot");

    assert_eq!(result.stream_position, position(2));
    assert_eq!(event_store.reads_from(), vec![ReadFrom::Beginning]);
    let snapshot_id = WasmSnapshotId::new(module.name(), module.version(), SCHEDULE_ID);
    assert!(snapshot_store.get(snapshot_id.as_str()).is_some());
}

#[tokio::test]
async fn a_failing_snapshot_write_does_not_fail_the_command() {
    let module = schedules_module();
    let event_store = InMemoryEventStore::default();
    let snapshot_store = InMemorySnapshotStore::default();
    let scheduler = ImmediateSnapshotTaskScheduler;
    snapshot_store.fail_writes();

    let result = WasmCommandExecution::new(&module, &event_store, &create_command(SCHEDULE_ID))
        .with_snapshot_store(&snapshot_store, &scheduler)
        .with_snapshot_cadence(SnapshotCadence::every_events(1))
        .execute()
        .await
        .expect("create succeeds even when the snapshot write fails");

    assert_eq!(result.stream_position, position(1));
    let snapshot_id = WasmSnapshotId::new(module.name(), module.version(), SCHEDULE_ID);
    assert!(snapshot_store.get(snapshot_id.as_str()).is_none());
}

#[tokio::test]
async fn a_replay_limit_bounds_the_stream_read() {
    let module = schedules_module();
    let event_store = InMemoryEventStore::default();

    WasmCommandExecution::new(&module, &event_store, &create_command(SCHEDULE_ID))
        .execute()
        .await
        .expect("create succeeds");

    WasmCommandExecution::new(&module, &event_store, &pause_command(SCHEDULE_ID))
        .with_replay_limit(replay_limit(8))
        .execute()
        .await
        .expect("pause stays within the replay limit");

    assert_eq!(event_store.read_bounds(), vec![Some(9)]);
}

#[tokio::test]
async fn a_replay_limit_bounds_the_stream_read_behind_a_snapshot_store() {
    let module = schedules_module();
    let event_store = InMemoryEventStore::default();
    let snapshot_store = InMemorySnapshotStore::default();
    let scheduler = ImmediateSnapshotTaskScheduler;

    WasmCommandExecution::new(&module, &event_store, &create_command(SCHEDULE_ID))
        .with_snapshot_store(&snapshot_store, &scheduler)
        .with_snapshot_cadence(SnapshotCadence::every_events(1))
        .execute()
        .await
        .expect("create succeeds");

    WasmCommandExecution::new(&module, &event_store, &pause_command(SCHEDULE_ID))
        .with_snapshot_store(&snapshot_store, &scheduler)
        .with_replay_limit(replay_limit(8))
        .execute()
        .await
        .expect("pause resumed from the snapshot stays within the replay limit");

    let expected_resume = ReadFrom::after(position(1)).expect("resume position advances");
    assert_eq!(event_store.reads_from(), vec![expected_resume]);
    assert_eq!(
        event_store.read_bounds(),
        vec![Some(9)],
        "the snapshot path must bound its read like the store-less path does"
    );
}

#[tokio::test]
async fn a_replay_limit_bounds_a_discard_and_replay_read() {
    let module = schedules_module();
    let event_store = InMemoryEventStore::default();
    let snapshot_store = InMemorySnapshotStore::default();
    let scheduler = ImmediateSnapshotTaskScheduler;

    WasmCommandExecution::new(&module, &event_store, &create_command(SCHEDULE_ID))
        .execute()
        .await
        .expect("create succeeds");

    let snapshot_id = WasmSnapshotId::new(module.name(), module.version(), SCHEDULE_ID);
    snapshot_store.insert(
        snapshot_id.as_str(),
        trogon_decider_runtime::Snapshot::new(position(5), OpaqueSnapshotPayload::new(Vec::new())),
    );

    WasmCommandExecution::new(&module, &event_store, &pause_command(SCHEDULE_ID))
        .with_snapshot_store(&snapshot_store, &scheduler)
        .with_snapshot_failure_policy(DiscardAndReplaySnapshotFailure)
        .with_replay_limit(replay_limit(8))
        .execute()
        .await
        .expect("discard-and-replay recovers from the ahead-of-stream snapshot");

    assert_eq!(
        event_store.read_bounds(),
        vec![Some(9), Some(9)],
        "the recovery replay is bounded by the same limit as the read it replaces"
    );
}

#[tokio::test]
async fn a_stream_past_the_replay_limit_fails_before_reaching_the_guest() {
    let module = schedules_module();
    let event_store = InMemoryEventStore::default();

    WasmCommandExecution::new(&module, &event_store, &create_command(SCHEDULE_ID))
        .execute()
        .await
        .expect("create succeeds");
    WasmCommandExecution::new(&module, &event_store, &pause_command(SCHEDULE_ID))
        .execute()
        .await
        .expect("pause succeeds");

    let Err(error) = WasmCommandExecution::new(&module, &event_store, &pause_command(SCHEDULE_ID))
        .with_replay_limit(replay_limit(1))
        .execute()
        .await
    else {
        panic!("expected the replay limit to reject the command");
    };

    assert!(matches!(error, WasmCommandError::ReplayLimitExceeded(_)), "{error}");
    assert_eq!(
        event_store.read_bounds(),
        vec![None, Some(2)],
        "reading one past the limit is enough to prove the stream exceeded it"
    );
    assert_eq!(
        event_store.write_preconditions(),
        vec![
            StreamWritePrecondition::NoStream,
            StreamWritePrecondition::At(position(1))
        ],
        "the rejected command must not append"
    );
}

/// Creates, pauses, resumes, and pauses a schedule, leaving four events to replay.
async fn four_event_history(module: &WasmDeciderModule, event_store: &InMemoryEventStore) {
    for command in [
        create_command(SCHEDULE_ID),
        pause_command(SCHEDULE_ID),
        resume_command(SCHEDULE_ID),
        pause_command(SCHEDULE_ID),
    ] {
        WasmCommandExecution::new(module, event_store, &command)
            .execute()
            .await
            .expect("history builds");
    }
}

#[tokio::test]
async fn a_replay_chunk_size_walks_the_stream_in_bounded_reads() {
    let module = schedules_module();
    let event_store = InMemoryEventStore::default();
    four_event_history(&module, &event_store).await;
    let before = event_store.read_stream_calls();

    let result = WasmCommandExecution::new(&module, &event_store, &resume_command(SCHEDULE_ID))
        .with_replay_chunk_size(chunk_size(2))
        .execute()
        .await
        .expect("resume folds every chunk of the history");

    assert_eq!(result.stream_position, position(5));
    assert_eq!(
        event_store.reads_from()[before..],
        [ReadFrom::Beginning, ReadFrom::Position(position(3))]
    );
    assert_eq!(event_store.read_bounds()[before..], [Some(2), Some(2)]);
    assert_eq!(
        event_store.write_preconditions().last().copied(),
        Some(StreamWritePrecondition::At(position(4))),
        "the append is guarded on the tail the walk was pinned to"
    );
}

#[tokio::test]
async fn a_chunk_of_one_folds_the_same_state_a_single_read_would() {
    let module = schedules_module();
    let event_store = InMemoryEventStore::default();
    four_event_history(&module, &event_store).await;
    let before = event_store.read_stream_calls();

    let result = WasmCommandExecution::new(&module, &event_store, &resume_command(SCHEDULE_ID))
        .with_replay_chunk_size(chunk_size(1))
        .execute()
        .await
        .expect("one evolve call per event reaches the same state as one call for all of them");

    assert_eq!(result.stream_position, position(5));
    assert_eq!(
        event_store.reads_from()[before..],
        [
            ReadFrom::Beginning,
            ReadFrom::Position(position(2)),
            ReadFrom::Position(position(3)),
            ReadFrom::Position(position(4)),
        ]
    );
    assert_eq!(
        event_store.read_bounds()[before..],
        [Some(1), Some(1), Some(1), Some(1)]
    );
}

#[tokio::test]
async fn a_chunk_size_and_a_limit_read_by_whichever_is_tighter() {
    let module = schedules_module();
    let event_store = InMemoryEventStore::default();
    four_event_history(&module, &event_store).await;
    let before = event_store.read_stream_calls();

    let Err(error) = WasmCommandExecution::new(&module, &event_store, &resume_command(SCHEDULE_ID))
        .with_replay_limit(replay_limit(3))
        .with_replay_chunk_size(chunk_size(2))
        .execute()
        .await
    else {
        panic!("expected the replay limit to reject the command");
    };

    assert!(matches!(error, WasmCommandError::ReplayLimitExceeded(_)), "{error}");
    assert_eq!(
        event_store.read_bounds()[before..],
        [Some(2), Some(2)],
        "the chunk caps the first read and the remaining allowance caps the second"
    );
    assert_eq!(
        event_store.write_preconditions().len(),
        4,
        "the rejected command must not append"
    );
}

#[tokio::test]
async fn a_chunked_replay_resumes_from_a_snapshot_and_still_walks_the_rest() {
    let module = schedules_module();
    let event_store = InMemoryEventStore::default();
    let snapshot_store = InMemorySnapshotStore::default();
    let scheduler = ImmediateSnapshotTaskScheduler;

    WasmCommandExecution::new(&module, &event_store, &create_command(SCHEDULE_ID))
        .with_snapshot_store(&snapshot_store, &scheduler)
        .with_snapshot_cadence(SnapshotCadence::every_events(1))
        .execute()
        .await
        .expect("create succeeds");
    for command in [
        pause_command(SCHEDULE_ID),
        resume_command(SCHEDULE_ID),
        pause_command(SCHEDULE_ID),
    ] {
        WasmCommandExecution::new(&module, &event_store, &command)
            .execute()
            .await
            .expect("history builds");
    }
    let before = event_store.read_stream_calls();

    let result = WasmCommandExecution::new(&module, &event_store, &resume_command(SCHEDULE_ID))
        .with_snapshot_store(&snapshot_store, &scheduler)
        .with_replay_chunk_size(chunk_size(2))
        .execute()
        .await
        .expect("the walk resumes where the snapshot left off");

    assert_eq!(result.stream_position, position(5));
    assert_eq!(
        event_store.reads_from()[before..],
        [ReadFrom::Position(position(2)), ReadFrom::Position(position(4))],
        "the snapshot decides where the walk starts, the chunk size decides how far each step goes"
    );
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

    let create_result = WasmCommandExecution::new(&module, &event_store, &create_command(SCHEDULE_ID))
        .execute()
        .await
        .expect("create succeeds under the pooling allocator");
    assert_eq!(create_result.stream_position, position(1));

    let pause_result = WasmCommandExecution::new(&module, &event_store, &pause_command(SCHEDULE_ID))
        .execute()
        .await
        .expect("pause succeeds under the pooling allocator");
    assert_eq!(pause_result.stream_position, position(2));
}

fn admission_limit(value: usize) -> AdmissionLimit {
    AdmissionLimit::try_new(value).expect("test admission limit must be non-zero")
}

#[tokio::test]
async fn an_unconfigured_execution_is_never_shed() {
    let module = schedules_module();
    let event_store = InMemoryEventStore::default();

    WasmCommandExecution::new(&module, &event_store, &create_command(SCHEDULE_ID))
        .execute()
        .await
        .expect("create succeeds");
    WasmCommandExecution::new(&module, &event_store, &pause_command(SCHEDULE_ID))
        .execute()
        .await
        .expect("pause succeeds without any admission slot to claim");
}

#[tokio::test]
async fn a_shed_command_never_reaches_the_guest() {
    let module = schedules_module();
    let event_store = InMemoryEventStore::default();
    let admission = ConcurrencyAdmission::new(admission_limit(1));
    let _held = admission.admit().expect("the only slot is free");

    let Err(error) = WasmCommandExecution::new(&module, &event_store, &create_command(SCHEDULE_ID))
        .with_admission(&admission)
        .execute()
        .await
    else {
        panic!("expected the committed slot to shed the command");
    };

    assert!(matches!(error, WasmCommandError::Overloaded(_)), "{error}");
    assert_eq!(
        event_store.reads_from(),
        Vec::new(),
        "shedding happens before the stream is read, so no wasm store was ever created"
    );
    assert_eq!(event_store.write_preconditions(), Vec::new());
}

#[tokio::test]
async fn an_execution_releases_its_admission_slot_when_it_ends() {
    let module = schedules_module();
    let event_store = InMemoryEventStore::default();
    let admission = ConcurrencyAdmission::new(admission_limit(1));

    WasmCommandExecution::new(&module, &event_store, &create_command(SCHEDULE_ID))
        .with_admission(&admission)
        .execute()
        .await
        .expect("create succeeds");

    assert_eq!(admission.in_flight(), 0);

    WasmCommandExecution::new(&module, &event_store, &pause_command(SCHEDULE_ID))
        .with_admission(&admission)
        .execute()
        .await
        .expect("the slot the create returned is reusable");
}

#[tokio::test]
async fn a_failed_execution_releases_its_admission_slot() {
    let module = schedules_module();
    let event_store = InMemoryEventStore::default();
    let admission = ConcurrencyAdmission::new(admission_limit(1));

    let Err(error) = WasmCommandExecution::new(&module, &event_store, &pause_command(MISSING_SCHEDULE_ID))
        .with_admission(&admission)
        .execute()
        .await
    else {
        panic!("expected rejection");
    };

    assert!(matches!(error, WasmCommandError::Rejected(_)), "{error}");
    assert_eq!(
        admission.in_flight(),
        0,
        "a slot released only on success would leak on every rejected command"
    );
}

/// An authorizer that grants on one claim and counts what it was asked.
#[derive(Debug)]
struct RequireClaim {
    claim: &'static str,
    calls: AtomicUsize,
}

impl RequireClaim {
    fn new(claim: &'static str) -> Self {
        Self {
            claim,
            calls: AtomicUsize::new(0),
        }
    }

    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

impl CommandAuthorizer<CommandEnvelope> for RequireClaim {
    fn authorize(
        &self,
        principal: &CommandPrincipal,
        _command: &CommandEnvelope,
    ) -> Result<(), AuthorizationDeniedError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if principal.has_claim(self.claim) {
            Ok(())
        } else {
            Err(AuthorizationDeniedError::new(format!("{} is required", self.claim)))
        }
    }
}

fn principal(id: &str, claims: &[&str]) -> CommandPrincipal {
    CommandPrincipal::new(PrincipalKind::Agent, PrincipalId::new(id).expect("test principal id")).with_claims(
        claims
            .iter()
            .map(|claim| PrincipalClaim::new(*claim).expect("test claim"))
            .collect(),
    )
}

#[tokio::test]
async fn an_unconfigured_execution_is_never_denied() {
    let module = schedules_module();
    let event_store = InMemoryEventStore::default();

    WasmCommandExecution::new(&module, &event_store, &create_command(SCHEDULE_ID))
        .execute()
        .await
        .expect("create succeeds with no principal to check");
}

#[tokio::test]
async fn a_denied_command_never_reaches_the_guest() {
    let module = schedules_module();
    let event_store = InMemoryEventStore::default();
    let authorizer = RequireClaim::new("schedules.write");

    let Err(error) = WasmCommandExecution::new(&module, &event_store, &create_command(SCHEDULE_ID))
        .with_principal(principal("agent-1", &["schedules.read"]))
        .with_authorizer(&authorizer)
        .execute()
        .await
    else {
        panic!("expected the missing claim to deny the command");
    };

    assert!(
        matches!(error, WasmCommandError::Unauthorized(UnauthorizedError::Denied(_))),
        "{error}"
    );
    assert_eq!(
        event_store.reads_from(),
        Vec::new(),
        "the denial lands before the guest computes a stream id, so nothing was read"
    );
    assert_eq!(event_store.write_preconditions(), Vec::new());
}

#[tokio::test]
async fn an_execution_with_an_authorizer_and_no_principal_is_denied() {
    let module = schedules_module();
    let event_store = InMemoryEventStore::default();
    let authorizer = RequireClaim::new("schedules.write");

    let Err(error) = WasmCommandExecution::new(&module, &event_store, &create_command(SCHEDULE_ID))
        .with_authorizer(&authorizer)
        .execute()
        .await
    else {
        panic!("expected an absent principal to be refused");
    };

    assert!(
        matches!(
            error,
            WasmCommandError::Unauthorized(UnauthorizedError::MissingPrincipal)
        ),
        "{error}"
    );
    assert_eq!(
        authorizer.calls(),
        0,
        "an absent principal is refused before any policy is consulted"
    );
    assert_eq!(event_store.write_preconditions(), Vec::new());
}

#[tokio::test]
async fn an_authorized_command_runs_as_it_would_unguarded() {
    let module = schedules_module();
    let event_store = InMemoryEventStore::default();
    let authorizer = RequireClaim::new("schedules.write");

    let result = WasmCommandExecution::new(&module, &event_store, &create_command(SCHEDULE_ID))
        .with_principal(principal("agent-1", &["schedules.write"]))
        .with_authorizer(&authorizer)
        .execute()
        .await
        .expect("the granted claim lets the command through");

    assert_eq!(result.stream_position, position(1));
    assert_eq!(authorizer.calls(), 1);
}

fn retry_limit(value: u32) -> ConflictRetryLimit {
    ConflictRetryLimit::try_new(value).expect("test retry limit must be non-zero")
}

#[tokio::test]
async fn a_conflict_within_the_budget_is_retried_until_the_append_lands() {
    let module = schedules_module();
    let event_store = ContendedEventStore::default();

    WasmCommandExecution::new(&module, &event_store, &create_command(SCHEDULE_ID))
        .execute()
        .await
        .expect("create succeeds");
    event_store.contend(2);
    let reads_before_pause = event_store.read_stream_calls();

    let result = WasmCommandExecution::new(&module, &event_store, &pause_command(SCHEDULE_ID))
        .with_conflict_retry(retry_limit(3))
        .execute()
        .await
        .expect("the pause outlasts the contention");

    assert_eq!(result.stream_position, position(2));
    assert_eq!(
        event_store.append_attempts(),
        vec![
            StreamWritePrecondition::NoStream,
            StreamWritePrecondition::At(position(1)),
            StreamWritePrecondition::At(position(1)),
            StreamWritePrecondition::At(position(1))
        ]
    );
    assert_eq!(
        event_store.read_stream_calls() - reads_before_pause,
        3,
        "a retry that skipped the replay would decide from state it already knows is stale"
    );
    assert_eq!(
        event_store.stored_event_types(SCHEDULE_ID).len(),
        2,
        "only the attempt that won the race may leave events behind"
    );
}

#[tokio::test]
async fn one_command_is_authorized_once_however_many_conflicts_it_survives() {
    let module = schedules_module();
    let event_store = ContendedEventStore::default();
    let authorizer = RequireClaim::new("schedules.write");

    WasmCommandExecution::new(&module, &event_store, &create_command(SCHEDULE_ID))
        .execute()
        .await
        .expect("create succeeds");
    event_store.contend(2);

    WasmCommandExecution::new(&module, &event_store, &pause_command(SCHEDULE_ID))
        .with_principal(principal("agent-1", &["schedules.write"]))
        .with_authorizer(&authorizer)
        .with_conflict_retry(retry_limit(3))
        .execute()
        .await
        .expect("the pause outlasts the contention");

    assert_eq!(
        authorizer.calls(),
        1,
        "a retry re-reads and re-decides, but it is still the one command the principal submitted"
    );
}

#[tokio::test]
async fn a_conflict_that_outlives_the_budget_reaches_the_caller() {
    let module = schedules_module();
    let event_store = ContendedEventStore::default();

    WasmCommandExecution::new(&module, &event_store, &create_command(SCHEDULE_ID))
        .execute()
        .await
        .expect("create succeeds");
    event_store.contend(5);

    let Err(error) = WasmCommandExecution::new(&module, &event_store, &pause_command(SCHEDULE_ID))
        .with_conflict_retry(retry_limit(2))
        .execute()
        .await
    else {
        panic!("expected the conflict to survive the budget");
    };

    assert!(matches!(error, WasmCommandError::Append(_)), "{error}");
    assert_eq!(
        event_store.append_attempts().len(),
        4,
        "one create, then two retries on top of the first pause attempt"
    );
}

#[tokio::test]
async fn without_a_configured_limit_the_first_conflict_is_the_answer() {
    let module = schedules_module();
    let event_store = ContendedEventStore::default();

    WasmCommandExecution::new(&module, &event_store, &create_command(SCHEDULE_ID))
        .execute()
        .await
        .expect("create succeeds");
    event_store.contend(1);

    let Err(error) = WasmCommandExecution::new(&module, &event_store, &pause_command(SCHEDULE_ID))
        .execute()
        .await
    else {
        panic!("expected the conflict to reach the caller");
    };

    assert!(matches!(error, WasmCommandError::Append(_)), "{error}");
    assert_eq!(event_store.append_attempts().len(), 2);
}

#[tokio::test]
async fn a_create_command_is_not_retried_past_its_own_precondition() {
    let module = schedules_module();
    let event_store = ContendedEventStore::default();
    event_store.contend(1);

    let Err(error) = WasmCommandExecution::new(&module, &event_store, &create_command(SCHEDULE_ID))
        .with_conflict_retry(retry_limit(3))
        .execute()
        .await
    else {
        panic!("expected the conflict to reach the caller");
    };

    assert!(matches!(error, WasmCommandError::Append(_)), "{error}");
    assert_eq!(
        event_store.append_attempts().len(),
        1,
        "re-reading cannot make a stream that now exists stop existing"
    );
}

#[tokio::test]
async fn a_caller_supplied_revision_is_not_retried_past() {
    let module = schedules_module();
    let event_store = ContendedEventStore::default();

    WasmCommandExecution::new(&module, &event_store, &create_command(SCHEDULE_ID))
        .execute()
        .await
        .expect("create succeeds");
    event_store.contend(1);

    let Err(error) = WasmCommandExecution::new(&module, &event_store, &pause_command(SCHEDULE_ID))
        .with_conflict_retry(retry_limit(3))
        .with_expected_revision(position(1))
        .execute()
        .await
    else {
        panic!("expected the conflict to reach the caller");
    };

    assert!(matches!(error, WasmCommandError::Append(_)), "{error}");
    assert_eq!(
        event_store.append_attempts().len(),
        2,
        "the conflict is the answer the caller asked for by naming a revision"
    );
}

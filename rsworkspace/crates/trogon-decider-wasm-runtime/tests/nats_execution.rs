//! Live JetStream coverage for WASM command execution storage semantics.
#![cfg(not(coverage))]
#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use std::fs;
use std::path::Path;

use async_nats::jetstream;
use buffa::Message as _;
use buffa::MessageName as _;
use trogon_decider_nats::{
    JetStreamStore, StreamStoreError, StreamSubject, StreamSubjectResolver, SubjectState, subject_current_position,
};
use trogon_decider_runtime::{
    DrainableSnapshotTaskScheduler, ReadFrom, ReadSnapshotRequest, ReadStreamRequest, SnapshotRead,
    SnapshotTaskScheduler, StreamRead, StreamWritePrecondition,
};
use trogon_decider_wasm_runtime::{
    OpaqueSnapshotPayload, WasmCommandError, WasmCommandExecution, WasmDeciderEngine, WasmDeciderModule,
    WasmEngineConfig, WasmSnapshotId,
};
use trogon_decider_wit::host::CommandEnvelope;
use trogon_nats::test_support::JetStreamTestServer;
use trogonai_proto::scheduler::schedules::{CREATE_SCHEDULE_TYPE_URL, PAUSE_SCHEDULE_TYPE_URL, v1};

const EVENTS_STREAM: &str = "WASM_EXECUTION_EVENTS";
const EVENTS_SUBJECT: &str = "wasm.execution.events.>";
const SNAPSHOT_BUCKET: &str = "WASM_EXECUTION_SNAPSHOTS";

#[derive(Clone, Copy)]
struct TestSubjectResolver;

impl StreamSubjectResolver<str> for TestSubjectResolver {
    type Error = StreamStoreError;

    async fn resolve_subject_state(
        &self,
        events_stream: &jetstream::stream::Stream,
        stream_id: &str,
    ) -> Result<SubjectState, Self::Error> {
        let subject = StreamSubject::new(format!("wasm.execution.events.{stream_id}"))
            .expect("test stream id produces a valid NATS subject");
        let current_position = subject_current_position(events_stream, &subject).await?;
        Ok(SubjectState {
            subject,
            current_position,
        })
    }
}

fn schedules_module() -> WasmDeciderModule {
    let relative = "../../target/wasm32-unknown-unknown/release/trogon_schedules_decider.wasm";
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(relative);
    let component = fs::read(&path).unwrap_or_else(|error| {
        panic!(
            "build trogon_schedules_decider.wasm for wasm32-unknown-unknown first (expected {}): {error}",
            path.display()
        )
    });
    let engine = WasmDeciderEngine::new(WasmEngineConfig::default()).expect("engine builds");
    WasmDeciderModule::load(engine, &component).expect("module loads")
}

fn create_command(id: &str) -> CommandEnvelope {
    CommandEnvelope {
        type_: CREATE_SCHEDULE_TYPE_URL.to_string(),
        payload: v1::CreateSchedule {
            schedule_id: id.to_string(),
            status: buffa::MessageField::some(v1::ScheduleStatus {
                kind: Some(v1::schedule_status::Scheduled {}.into()),
            }),
            schedule: buffa::MessageField::some(v1::Schedule {
                kind: Some(
                    v1::schedule::Every {
                        every: buffa::MessageField::some(buffa_types::google::protobuf::Duration {
                            seconds: 30,
                            nanos: 0,
                            ..buffa_types::google::protobuf::Duration::default()
                        }),
                    }
                    .into(),
                ),
            }),
            delivery: buffa::MessageField::some(v1::Delivery {
                kind: Some(
                    v1::delivery::NatsMessage {
                        subject: "agent.run".to_string(),
                        ttl: buffa::MessageField::none(),
                        source: buffa::MessageField::none(),
                    }
                    .into(),
                ),
            }),
            message: buffa::MessageField::some(v1::Message {
                content: buffa::MessageField::some(trogonai_proto::content::v1alpha1::Content {
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

async fn live_store() -> (JetStreamTestServer, JetStreamStore<TestSubjectResolver>) {
    let server = JetStreamTestServer::start().await;
    let js = server.jetstream().await;
    let events_stream = js
        .create_stream(jetstream::stream::Config {
            name: EVENTS_STREAM.to_string(),
            subjects: vec![EVENTS_SUBJECT.to_string()],
            allow_atomic_publish: true,
            ..Default::default()
        })
        .await
        .expect("create events stream");
    let snapshot_bucket = js
        .create_key_value(jetstream::kv::Config {
            bucket: SNAPSHOT_BUCKET.to_string(),
            history: 1,
            ..Default::default()
        })
        .await
        .expect("create snapshot bucket");
    let store = JetStreamStore::builder(js, events_stream, snapshot_bucket).with_subject_resolver(TestSubjectResolver);
    (server, store)
}

async fn assert_only_creation_is_stored(store: &JetStreamStore<TestSubjectResolver>, stream_id: &str) {
    let replay = store
        .read_stream(ReadStreamRequest {
            stream_id,
            from: ReadFrom::Beginning,
        })
        .await
        .expect("read live event stream");
    assert_eq!(replay.events.len(), 1);
    assert_eq!(replay.events[0].event.r#type, v1::ScheduleCreated::FULL_NAME);
}

#[tokio::test]
async fn builder_no_stream_skips_live_jetstream_replay() {
    let module = schedules_module();
    let (_server, store) = live_store().await;

    WasmCommandExecution::new(&module, &store, &create_command("without-snapshot"))
        .execute()
        .await
        .expect("seed schedule history in JetStream");

    let Err(error) = WasmCommandExecution::new(&module, &store, &pause_command("without-snapshot"))
        .with_write_precondition(StreamWritePrecondition::NoStream)
        .execute()
        .await
    else {
        panic!("pause unexpectedly succeeded");
    };
    assert!(matches!(error, WasmCommandError::Rejected(_)), "{error}");
    assert_only_creation_is_stored(&store, "without-snapshot").await;
}

#[tokio::test]
async fn builder_no_stream_skips_live_jetstream_snapshot_and_replay() {
    let module = schedules_module();
    let (_server, store) = live_store().await;

    let snapshot_scheduler = DrainableSnapshotTaskScheduler::new();
    WasmCommandExecution::new(&module, &store, &create_command("with-snapshot"))
        .with_snapshot_store(&store, &snapshot_scheduler)
        .execute()
        .await
        .expect("seed schedule history and snapshot in JetStream");
    snapshot_scheduler.drain().await;
    let snapshot_id = WasmSnapshotId::new(module.name(), module.version(), "with-snapshot");
    let snapshot = <JetStreamStore<TestSubjectResolver> as SnapshotRead<OpaqueSnapshotPayload, str>>::read_snapshot(
        &store,
        ReadSnapshotRequest {
            snapshot_id: snapshot_id.as_str(),
        },
    )
    .await
    .expect("read seeded snapshot from JetStream");
    assert!(snapshot.snapshot.is_some());

    let Err(error) = WasmCommandExecution::new(&module, &store, &pause_command("with-snapshot"))
        .with_snapshot_store(&store, &snapshot_scheduler)
        .with_write_precondition(StreamWritePrecondition::NoStream)
        .execute()
        .await
    else {
        panic!("pause unexpectedly succeeded");
    };
    assert!(matches!(error, WasmCommandError::Rejected(_)), "{error}");
    assert_only_creation_is_stored(&store, "with-snapshot").await;
}

use buffa::Message as _;
use buffa::MessageField;
use trogon_decider_runtime::{ImmediateSnapshotTaskScheduler, ReadFrom, SnapshotCadence, StreamPosition};
use trogon_decider_wit::host::CommandEnvelope;
use trogonai_proto::content::v1alpha1 as content_v1alpha1;
use trogonai_proto::scheduler::schedules::v1;

use async_nats::jetstream;
use buffa::MessageName as _;
use trogon_decider_nats::{
    JetStreamStore, StreamStoreError, StreamSubject, StreamSubjectResolver, SubjectState, subject_current_position,
};
use trogon_decider_runtime::{ReadStreamRequest, StreamRead};
use trogon_nats::test_support::JetStreamTestServer;

use super::*;
use crate::test_doubles::{InMemoryEventStore, InMemorySnapshotStore};
use crate::test_fixture::schedules_bytes;
use crate::{WasmCommandExecution, WasmDeciderEngine, WasmEngineConfig, WasmSnapshotId};

const SCHEDULE_ID: &str = "0198be07a38479e1a376f250f9181be9";
const HOT_SWAP_SCHEDULE_ID: &str = "0198be07a38479e1a376f250f9181bee";
const HOT_SWAP_EVENTS_STREAM: &str = "WASM_REGISTRY_HOT_SWAP_EVENTS";
const HOT_SWAP_EVENTS_SUBJECT: &str = "wasm.registry.hot_swap.events.>";
const HOT_SWAP_SNAPSHOT_BUCKET: &str = "WASM_REGISTRY_HOT_SWAP_SNAPSHOTS";

fn schedules_module() -> WasmDeciderModule {
    let engine = WasmDeciderEngine::new(WasmEngineConfig::default()).expect("engine builds");
    WasmDeciderModule::load(engine, &schedules_bytes()).expect("module loads")
}

fn create_type() -> CommandType {
    CommandType::new(trogonai_proto::scheduler::schedules::CREATE_SCHEDULE_TYPE_URL).expect("valid command type")
}

fn pause_type() -> CommandType {
    CommandType::new(trogonai_proto::scheduler::schedules::PAUSE_SCHEDULE_TYPE_URL).expect("valid command type")
}

fn create_command(id: &str) -> CommandEnvelope {
    CommandEnvelope {
        type_: trogonai_proto::scheduler::schedules::CREATE_SCHEDULE_TYPE_URL.to_string(),
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
        type_: trogonai_proto::scheduler::schedules::PAUSE_SCHEDULE_TYPE_URL.to_string(),
        payload: v1::PauseSchedule {
            schedule_id: id.to_string(),
        }
        .encode_to_vec(),
    }
}

fn position(value: u64) -> StreamPosition {
    StreamPosition::try_new(value).expect("test stream position must be non-zero")
}

#[derive(Clone, Copy)]
struct TestSubjectResolver;

impl StreamSubjectResolver<str> for TestSubjectResolver {
    type Error = StreamStoreError;

    async fn resolve_subject_state(
        &self,
        events_stream: &jetstream::stream::Stream,
        stream_id: &str,
    ) -> Result<SubjectState, Self::Error> {
        let subject = StreamSubject::new(format!("wasm.registry.hot_swap.events.{stream_id}"))
            .expect("test stream id produces a valid NATS subject");
        let current_position = subject_current_position(events_stream, &subject).await?;
        Ok(SubjectState {
            subject,
            current_position,
        })
    }
}

async fn live_hot_swap_store() -> (JetStreamTestServer, JetStreamStore<TestSubjectResolver>) {
    let server = JetStreamTestServer::start().await;
    let js = server.jetstream().await;
    let events_stream = js
        .create_stream(jetstream::stream::Config {
            name: HOT_SWAP_EVENTS_STREAM.to_string(),
            subjects: vec![HOT_SWAP_EVENTS_SUBJECT.to_string()],
            allow_atomic_publish: true,
            ..Default::default()
        })
        .await
        .expect("create events stream");
    let snapshot_bucket = js
        .create_key_value(jetstream::kv::Config {
            bucket: HOT_SWAP_SNAPSHOT_BUCKET.to_string(),
            history: 1,
            ..Default::default()
        })
        .await
        .expect("create snapshot bucket");
    let store = JetStreamStore::builder(js, events_stream, snapshot_bucket).with_subject_resolver(TestSubjectResolver);
    (server, store)
}

#[test]
fn routes_registered_command_types() {
    let registry = DeciderRegistry::builder()
        .register(schedules_module())
        .expect("registration succeeds")
        .build();

    let command_type =
        CommandType::new(trogonai_proto::scheduler::schedules::CREATE_SCHEDULE_TYPE_URL).expect("valid command type");
    let module = registry.route(&command_type).expect("route succeeds");
    assert_eq!(module.name().as_str(), "scheduler.schedules");
}

#[test]
fn unknown_command_type_is_reported() {
    let registry = DeciderRegistry::builder()
        .register(schedules_module())
        .expect("registration succeeds")
        .build();

    let command_type = CommandType::new("NoSuchCommand").expect("valid command type");
    let Err(error) = registry.route(&command_type) else {
        panic!("expected unknown command type error");
    };
    assert_eq!(error.command_type, command_type);
}

#[test]
fn duplicate_command_type_across_modules_is_rejected() {
    let builder = DeciderRegistry::builder()
        .register(schedules_module())
        .expect("first registration succeeds");

    let Err(error) = builder.register(schedules_module()) else {
        panic!("expected duplicate command type error");
    };
    assert_eq!(error.existing_module.as_str(), "scheduler.schedules");
    assert_eq!(error.new_module.as_str(), "scheduler.schedules");
}

#[test]
fn routes_reports_every_registered_command_type_with_module_identity() {
    let registry = DeciderRegistry::builder()
        .register(schedules_module())
        .expect("registration succeeds")
        .build();

    let mut routes = registry.routes();
    routes.sort();

    assert_eq!(routes.len(), 4);
    assert!(
        routes
            .iter()
            .all(|route| route.module_name.as_str() == "scheduler.schedules")
    );
    assert!(routes.iter().any(|route| route.command_type == create_type()));
    assert!(routes.iter().any(|route| route.command_type == pause_type()));
}

#[test]
fn handle_routes_delegate_to_the_wrapped_registry() {
    let registry = DeciderRegistry::builder()
        .register(schedules_module())
        .expect("registration succeeds")
        .build();
    let mut expected_routes = registry.routes();
    expected_routes.sort();

    let handle = DeciderRegistryHandle::from(registry);

    let mut routes = handle.routes();
    routes.sort();
    assert_eq!(routes, expected_routes);

    let module = handle.route(&create_type()).expect("route succeeds");
    assert_eq!(module.name().as_str(), "scheduler.schedules");
    assert_eq!(handle.snapshot().routes().len(), 4);
}

#[test]
fn activate_adds_routes_for_every_command_type_the_module_declares() {
    let handle = DeciderRegistryHandle::default();

    let activated = handle.activate(schedules_module());

    assert_eq!(activated.len(), 4);
    assert!(activated.iter().all(|route| route.previous_module.is_none()));
    assert!(activated.iter().any(|route| route.command_type == create_type()));
    assert!(activated.iter().any(|route| route.command_type == pause_type()));

    let module = handle.route(&create_type()).expect("route succeeds");
    assert_eq!(module.name().as_str(), "scheduler.schedules");
}

#[test]
fn activate_replaces_the_current_route_and_reports_the_previous_module_identity() {
    let handle = DeciderRegistryHandle::default();
    let module_v1 = schedules_module();
    let v1_version = module_v1.version().clone();
    handle.activate(module_v1.clone());

    let module_v2 = module_v1.with_version(ModuleVersion::new("0.2.0").expect("valid version"));
    let activated = handle.activate(module_v2);

    assert_eq!(activated.len(), 4);
    for route in &activated {
        let previous = route
            .previous_module
            .as_ref()
            .expect("a previous module owned this route");
        assert_eq!(previous.0.as_str(), "scheduler.schedules");
        assert_eq!(previous.1, v1_version);
    }

    let module = handle.route(&create_type()).expect("route succeeds");
    assert_eq!(module.version().as_str(), "0.2.0");
}

#[test]
fn retire_removes_the_route_and_reports_the_retired_module_identity() {
    let handle = DeciderRegistryHandle::default();
    handle.activate(schedules_module());

    let retired = handle.retire(&create_type()).expect("create was routed");
    assert_eq!(retired.command_type, create_type());
    assert_eq!(retired.module_name.as_str(), "scheduler.schedules");

    assert!(handle.route(&create_type()).is_err());
    assert!(handle.route(&pause_type()).is_ok());
    assert!(handle.retire(&create_type()).is_none());
}

#[test]
fn concurrent_activation_never_exposes_a_torn_route_pair() {
    let module_v1 = schedules_module();
    let module_v2 = module_v1
        .clone()
        .with_version(ModuleVersion::new("0.2.0").expect("valid version"));

    let handle = DeciderRegistryHandle::default();
    handle.activate(module_v1.clone());

    let handle_ref = &handle;
    std::thread::scope(|scope| {
        scope.spawn(move || {
            for _ in 0..200 {
                handle_ref.activate(module_v1.clone());
                handle_ref.activate(module_v2.clone());
            }
        });

        for _ in 0..200 {
            // Pinning one snapshot before reading both routes is essential here: two
            // separate `handle.route()` calls each take their own read lock, so a
            // swap between them could legitimately observe two different versions
            // without that meaning the swap itself was torn. Resolving through one
            // `Arc<DeciderRegistry>` is what this test needs to exercise.
            let registry = handle.snapshot();
            let create_version = registry
                .route(&create_type())
                .expect("route succeeds")
                .version()
                .clone();
            let pause_version = registry.route(&pause_type()).expect("route succeeds").version().clone();
            assert_eq!(
                create_version, pause_version,
                "a single resolved registry snapshot must never disagree with itself about which module \
                 version owns two command types declared by the same module"
            );
        }
    });
}

#[tokio::test]
async fn activating_a_new_module_version_starts_cold_and_keeps_the_prior_snapshot() {
    let module_v1 = schedules_module();
    let module_v2 = module_v1
        .clone()
        .with_version(ModuleVersion::new("0.2.0").expect("valid version"));

    let handle = DeciderRegistry::builder()
        .register(module_v1)
        .expect("registration succeeds")
        .build_handle();

    let event_store = InMemoryEventStore::default();
    let snapshot_store = InMemorySnapshotStore::default();
    let scheduler = ImmediateSnapshotTaskScheduler;

    let v1_create_module = handle.route(&create_type()).expect("v1 routes create");
    WasmCommandExecution::new(&v1_create_module, &event_store, &create_command(SCHEDULE_ID))
        .with_snapshot_store(&snapshot_store, &scheduler)
        .with_snapshot_cadence(SnapshotCadence::every_events(1))
        .execute()
        .await
        .expect("create succeeds against v1");

    let v1_snapshot_id = WasmSnapshotId::new(v1_create_module.name(), v1_create_module.version(), SCHEDULE_ID);
    assert_eq!(
        snapshot_store
            .get(v1_snapshot_id.as_str())
            .expect("v1 must have written a snapshot")
            .position,
        position(1)
    );

    let activated = handle.activate(module_v2);
    assert_eq!(activated.len(), 4);

    let v2_pause_module = handle.route(&pause_type()).expect("v2 now routes pause");
    assert_eq!(v2_pause_module.version().as_str(), "0.2.0");

    let result = WasmCommandExecution::new(&v2_pause_module, &event_store, &pause_command(SCHEDULE_ID))
        .with_snapshot_store(&snapshot_store, &scheduler)
        .with_snapshot_cadence(SnapshotCadence::every_events(1))
        .execute()
        .await
        .expect("pause against v2 replays from the beginning instead of resuming a v1 snapshot");

    assert_eq!(result.stream_position, position(2));
    assert_eq!(
        event_store.reads_from(),
        vec![ReadFrom::Beginning],
        "v2 has no snapshot of its own yet, so it must do a full replay rather than resume v1's snapshot position"
    );

    let v2_snapshot_id = WasmSnapshotId::new(v2_pause_module.name(), v2_pause_module.version(), SCHEDULE_ID);
    assert_ne!(v1_snapshot_id, v2_snapshot_id);
    assert!(
        snapshot_store.get(v1_snapshot_id.as_str()).is_some(),
        "v1's snapshot must survive the version swap untouched"
    );
    assert_eq!(
        snapshot_store
            .get(v2_snapshot_id.as_str())
            .expect("v2 must write its own snapshot side by side with v1's")
            .position,
        position(2)
    );
}

#[tokio::test]
async fn activate_under_live_jetstream_keeps_in_flight_dispatch_pinned_to_its_resolved_module() {
    let (_server, store) = live_hot_swap_store().await;

    let module_v1 = schedules_module();
    let module_v2 = module_v1
        .clone()
        .with_version(ModuleVersion::new("0.2.0").expect("valid version"));

    let handle = DeciderRegistry::builder()
        .register(module_v1)
        .expect("registration succeeds")
        .build_handle();

    let in_flight_module = handle.route(&create_type()).expect("v1 routes create");

    let activated = handle.activate(module_v2);
    assert_eq!(activated.len(), 4);

    WasmCommandExecution::new(&in_flight_module, &store, &create_command(HOT_SWAP_SCHEDULE_ID))
        .execute()
        .await
        .expect("dispatch resolved before the swap still completes against its pinned v1 module");
    assert_eq!(
        in_flight_module.version().as_str(),
        "0.1.0",
        "the module an in-flight dispatch already resolved must not change underneath it"
    );

    let next_dispatch_module = handle.route(&pause_type()).expect("v2 now routes pause");
    assert!(
        !Arc::ptr_eq(&in_flight_module, &next_dispatch_module),
        "a dispatch resolving after the swap must observe the newly activated module, not the pinned one"
    );
    assert_eq!(next_dispatch_module.version().as_str(), "0.2.0");

    let result = WasmCommandExecution::new(&next_dispatch_module, &store, &pause_command(HOT_SWAP_SCHEDULE_ID))
        .execute()
        .await
        .expect("the next dispatch resolves and executes against the swapped-in v2 module");
    assert_eq!(result.stream_position, position(2));

    let replay = store
        .read_stream(ReadStreamRequest {
            stream_id: HOT_SWAP_SCHEDULE_ID,
            from: ReadFrom::Beginning,
        })
        .await
        .expect("read live event stream");
    assert_eq!(replay.events.len(), 2);
    assert_eq!(replay.events[0].event.r#type, v1::ScheduleCreated::FULL_NAME);
    assert_eq!(replay.events[1].event.r#type, v1::SchedulePaused::FULL_NAME);
}

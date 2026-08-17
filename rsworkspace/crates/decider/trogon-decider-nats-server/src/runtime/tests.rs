use std::time::Duration;

use trogon_decider_nats::DuplicateWindow;
use trogon_decider_runtime::AdmissionLimit;
use trogon_semconv::attribute::DecisionOutcome;
use trogonai_proto::decider::CommandOutcomeCase;
use trogonai_proto::google::rpc::ErrorInfo;

use crate::command_subject::SubjectPrefix;
use crate::config::ModuleStore;
use crate::constants::{
    CONTENT_TYPE_HEADER, DEFAULT_EVENTS_STREAM, DEFAULT_SNAPSHOT_BUCKET, DEFAULT_SUBJECT_PREFIX,
    TROGON_COMMAND_ID_HEADER,
};
use crate::status::{FaultClass, find_detail};

use super::*;

fn subjects() -> CommandSubjects {
    CommandSubjects::new(SubjectPrefix::new(DEFAULT_SUBJECT_PREFIX).expect("the default prefix is a token"))
}

fn router() -> CommandRouter {
    CommandRouter::new(subjects(), Arc::new(DeciderRegistryHandle::default()))
}

fn config() -> ServerConfig {
    ServerConfig {
        subjects: subjects(),
        queue_group: "q".to_owned(),
        events_stream: DEFAULT_EVENTS_STREAM.to_owned(),
        snapshot_bucket: DEFAULT_SNAPSHOT_BUCKET.to_owned(),
        module_store: ModuleStore::Directory("/srv/modules".into()),
        modules: Vec::new(),
        admission_limit: AdmissionLimit::try_new(4).expect("four is not zero"),
        replay_limit: None,
        duplicate_window: DuplicateWindow::try_new(Duration::from_secs(120)).expect("two minutes is a window"),
    }
}

fn module_name(value: &str) -> ModuleName {
    ModuleName::new(value).expect("test module names are valid")
}

fn engine() -> WasmDeciderEngine {
    WasmDeciderEngine::new(WasmEngineConfig::default()).expect("the default engine config builds")
}

/// A store whose every fetch fails, so the startup path that reports an
/// unreachable module can be exercised without one.
struct UnreachableModules;

impl ModuleSource for UnreachableModules {
    type Error = std::io::Error;

    async fn fetch(&self, _reference: &ModuleReference) -> Result<Vec<u8>, Self::Error> {
        Err(std::io::Error::other("the store is unreachable"))
    }

    fn describe(&self) -> String {
        "unreachable store".to_owned()
    }
}

/// A store that answers with bytes that are not a component.
struct NotAComponent;

impl ModuleSource for NotAComponent {
    type Error = std::io::Error;

    async fn fetch(&self, _reference: &ModuleReference) -> Result<Vec<u8>, Self::Error> {
        Ok(b"\0asm not really a component".to_vec())
    }

    fn describe(&self) -> String {
        "fixture store".to_owned()
    }
}

fn reference(value: &str) -> ModuleReference {
    value.parse().expect("a well-formed reference")
}

fn fault_reason(reply: &CommandReply) -> String {
    let Some(CommandOutcomeCase::Faulted(status)) = reply.outcome().outcome.as_ref() else {
        panic!("expected a faulted reply, got {:?}", reply.outcome());
    };
    find_detail::<ErrorInfo>(status)
        .expect("every decider status names its reason")
        .reason
}

#[test]
fn a_command_no_module_claims_is_unroutable() {
    let reply = router()
        .route(
            "decider.trogonai.scheduler.schedules.v1.CreateSchedule",
            Vec::new(),
            None,
        )
        .expect_err("an empty registry claims nothing");

    assert_eq!(reply.decision(), DecisionOutcome::Faulted);
    assert_eq!(
        fault_reason(&reply),
        FaultClass::Unroutable.reason(),
        "a caller told 'internal' would retry forever; told 'unroutable' it goes and activates the module"
    );
}

#[test]
fn a_subject_outside_the_hosts_prefix_is_an_invalid_request() {
    let reply = router()
        .route(
            "elsewhere.trogonai.scheduler.schedules.v1.CreateSchedule",
            Vec::new(),
            None,
        )
        .expect_err("this host does not answer under that prefix");

    assert_eq!(fault_reason(&reply), FaultClass::InvalidRequest.reason());
}

#[test]
fn the_bare_prefix_names_no_command() {
    let reply = router()
        .route(DEFAULT_SUBJECT_PREFIX, Vec::new(), None)
        .expect_err("the subtree root is not a command");

    assert_eq!(fault_reason(&reply), FaultClass::InvalidRequest.reason());
}

#[test]
fn an_encoding_the_host_does_not_speak_is_refused_before_routing() {
    let mut headers = HeaderMap::new();
    headers.insert(CONTENT_TYPE_HEADER, "application/json");

    let reply = router()
        .route(
            "decider.trogonai.scheduler.schedules.v1.CreateSchedule",
            Vec::new(),
            Some(&headers),
        )
        .expect_err("json is not a decider command encoding");

    assert_eq!(
        fault_reason(&reply),
        FaultClass::InvalidRequest.reason(),
        "a caller speaking the wrong encoding has a bug in its client, not in the deployment"
    );
}

#[test]
fn an_unparseable_header_outranks_an_unroutable_subject() {
    let mut headers = HeaderMap::new();
    headers.insert(TROGON_COMMAND_ID_HEADER, "not-a-uuid");

    let reply = router()
        .route(
            "decider.trogonai.scheduler.schedules.v1.CreateSchedule",
            Vec::new(),
            Some(&headers),
        )
        .expect_err("a malformed command id is not a command");

    assert_eq!(
        fault_reason(&reply),
        FaultClass::InvalidRequest.reason(),
        "reporting 'unroutable' would send the caller to inspect a deployment that is fine"
    );
}

#[test]
fn no_routing_failure_ever_reports_as_a_decision() {
    let subjects = [
        "decider.trogonai.scheduler.schedules.v1.CreateSchedule",
        "elsewhere.some.Command",
        DEFAULT_SUBJECT_PREFIX,
    ];

    for subject in subjects {
        let reply = router()
            .route(subject, Vec::new(), None)
            .expect_err("nothing routes in an empty registry");

        assert_eq!(
            reply.header_value(),
            DecisionOutcome::Faulted.as_str(),
            "a header that disagreed with the body would meter '{subject}' as something the caller never saw"
        );
    }
}

#[test]
fn the_router_exposes_the_projection_and_the_routable_set_it_reads() {
    let router = router();

    assert_eq!(router.subjects().prefix().as_str(), DEFAULT_SUBJECT_PREFIX);
    assert!(
        router.registry().routes().is_empty(),
        "an empty registry claims nothing"
    );
}

#[test]
fn the_events_stream_captures_every_configured_modules_subtree() {
    let mut config = config();
    config.events_stream = "TENANT_EVENTS".to_owned();

    let stream = events_stream_config(
        &config,
        &[module_name("scheduler.schedules"), module_name("billing.invoices")],
    )
    .expect("both module names form a subtree");

    assert_eq!(stream.name, "TENANT_EVENTS");
    assert_eq!(
        stream.subjects,
        vec!["scheduler.schedules.events.>", "billing.invoices.events.>"],
        "a module the stream does not capture is a module whose events land nowhere"
    );
}

#[test]
fn the_events_stream_accepts_a_whole_decision_at_once() {
    let stream =
        events_stream_config(&config(), &[module_name("scheduler.schedules")]).expect("the module name is a subtree");

    assert!(
        stream.allow_atomic_publish,
        "without it a multi-event decision could land half-appended, which is a state no fold can read"
    );
}

#[test]
fn the_events_stream_carries_the_window_command_idempotency_rests_on() {
    let mut config = config();
    config.duplicate_window = DuplicateWindow::try_new(Duration::from_secs(30)).expect("thirty seconds is a window");

    let stream =
        events_stream_config(&config, &[module_name("scheduler.schedules")]).expect("the module name is a subtree");

    assert_eq!(stream.duplicate_window, Duration::from_secs(30));
}

#[test]
fn the_snapshot_bucket_keeps_only_the_revision_anyone_reads() {
    let mut config = config();
    config.snapshot_bucket = "TENANT_SNAPSHOTS".to_owned();

    let bucket = snapshot_bucket_config(&config);

    assert_eq!(bucket.bucket, "TENANT_SNAPSHOTS");
    assert_eq!(
        bucket.history, 1,
        "a snapshot is a cache of a fold, so every revision but the latest is storage nobody reads"
    );
}

#[tokio::test]
async fn a_module_the_store_cannot_produce_names_the_store_that_was_searched() {
    let Err(error) = load_modules(
        &engine(),
        &UnreachableModules,
        &[reference("scheduler.schedules@0.1.0")],
    )
    .await
    else {
        panic!("nothing is reachable there");
    };

    let StartupError::FetchModule { reference, store, .. } = error else {
        panic!("expected a fetch failure, got {error}");
    };
    assert_eq!(reference.to_string(), "scheduler.schedules@0.1.0");
    assert_eq!(
        store, "unreachable store",
        "an operator told only 'module not found' does not learn which store to go and look in"
    );
}

#[tokio::test]
async fn bytes_that_are_not_a_decider_component_stop_startup() {
    let Err(error) = load_modules(&engine(), &NotAComponent, &[reference("scheduler.schedules@0.1.0")]).await else {
        panic!("those bytes are not a component");
    };

    assert!(
        matches!(error, StartupError::LoadModule { .. }),
        "a host that started with an unloadable module would fail every command it routes there instead: {error}"
    );
}

#[tokio::test]
async fn a_host_configured_with_no_modules_loads_none() {
    let modules = load_modules(&engine(), &UnreachableModules, &[])
        .await
        .expect("an empty configuration asks the store for nothing");

    assert!(modules.is_empty());
}

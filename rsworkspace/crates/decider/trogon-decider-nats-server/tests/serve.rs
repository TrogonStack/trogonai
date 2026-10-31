//! End-to-end coverage for the ADR#0057 binding against a live NATS server.
//!
//! The unit tests prove each seam in isolation; this one proves they are wired
//! to each other. A crate whose parts are all individually correct and whose
//! serve loop never actually replies would pass every one of those tests.
#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use async_nats::jetstream::{self, message::StreamMessage};
use async_nats::{HeaderMap, RequestError, RequestErrorKind};
use buffa::Message as _;
use grpc_nats_micro::{
    ContentType, DiscoveryMetadata, EndpointBinding, MethodName, MethodNameInput, ServiceName, ServiceNameInput,
    SubjectPrefix as MicroSubjectPrefix, SubjectPrefixInput,
};
use tokio_util::sync::CancellationToken;
use trogon_decider_nats::DuplicateWindow;
use trogon_decider_nats_server::constants::{
    CONTENT_TYPE_HEADER, DECIDE_METHOD_NAME, DECIDER_SERVICE_NAME, DEFAULT_QUEUE_GROUP, DEFAULT_SUBJECT_PREFIX,
    NATS_SERVICE_ERROR_CODE_HEADER, PROTOBUF_CONTENT_TYPE, TYPE_URL_PREFIX,
};
use trogon_decider_nats_server::{
    CommandEndpoint, DeciderHost, FaultClass, FileModuleSource, ModuleReference, ModuleStore, PrincipalPrefix,
    ServerConfig, SubjectPrefix, TenantPosition, find_detail, serve_until,
};
use trogon_decider_runtime::{
    AdmissionLimit, AuthorizationDeniedError, CommandAuthorizer, CommandPrincipal, PrincipalId, PrincipalKind,
};
use trogon_decider_wit::host::CommandEnvelope;
use trogon_nats::test_support::JetStreamTestServer;
use trogonai_proto::decider::v1 as decider_v1;
use trogonai_proto::google::rpc::{Code, ErrorInfo, Status};
use trogonai_proto::scheduler::schedules::v1;

const EVENTS_STREAM: &str = "SERVE_TEST_EVENTS";
const SNAPSHOT_BUCKET: &str = "SERVE_TEST_SNAPSHOTS";
const CREATE_SCHEDULE: &str = "trogonai.scheduler.schedules.v1.CreateSchedule";
const MODULE_REFERENCE: &str = "scheduler.schedules@0.1.0";
const SCHEDULE_ID: &str = "0198be07a38479e1a376f250f9181bec";
const ALICE: &str = "alice";
const ALICE_SUBJECT: &str = "decider.agent.alice.DeciderService.Decide";
const MALLORY_SUBJECT: &str = "decider.agent.mallory.DeciderService.Decide";
const TENANT_ALICE_SUBJECT: &str = "acme.decider.agent.alice.DeciderService.Decide";
const FORGED_PRINCIPAL_HEADER: &str = "X-A2a-Caller-Id";
const UNDECODABLE_PAYLOAD: [u8; 4] = [0xff, 0xff, 0xff, 0xff];

fn module_path() -> PathBuf {
    let relative = "../../../target/wasm32-unknown-unknown/release/trogon_schedules_decider.wasm";
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(relative);
    assert!(
        path.exists(),
        "build trogon_schedules_decider.wasm for wasm32-unknown-unknown first (expected {})",
        path.display()
    );
    path
}

fn module_reference() -> ModuleReference {
    MODULE_REFERENCE.parse().expect("the fixture names itself this way")
}

/// Stages the prebuilt component under the name its reference resolves to.
///
/// The cargo artifact is named after the crate that produced it, and a host
/// that reads a store rather than a path only ever looks for the reference.
fn module_store() -> tempfile::TempDir {
    let root = tempfile::tempdir().expect("a temp dir");
    std::fs::copy(module_path(), root.path().join(module_reference().file_name())).expect("the component copies");
    root
}

fn endpoint(tenant: TenantPosition) -> CommandEndpoint {
    CommandEndpoint::new(
        SubjectPrefix::new(DEFAULT_SUBJECT_PREFIX).expect("the default is a token"),
        tenant,
    )
    .expect("the default prefix yields a conformant subject")
}

fn config(root: &Path, tenant: TenantPosition) -> ServerConfig {
    ServerConfig {
        endpoint: endpoint(tenant),
        queue_group: DEFAULT_QUEUE_GROUP.to_owned(),
        events_stream: EVENTS_STREAM.to_owned(),
        snapshot_bucket: SNAPSHOT_BUCKET.to_owned(),
        module_store: ModuleStore::Directory(root.to_path_buf()),
        modules: vec![module_reference()],
        admission_limit: AdmissionLimit::try_new(4).expect("four is non-zero"),
        replay_limit: None,
        duplicate_window: DuplicateWindow::try_new(Duration::from_secs(120)).expect("two minutes is enforceable"),
    }
}

fn alice_id() -> PrincipalId {
    PrincipalId::new(ALICE).expect("alice is a principal id")
}

fn alice() -> CommandPrincipal {
    CommandPrincipal::new(PrincipalKind::Agent, alice_id())
}

/// Admits one principal and denies every other, the way a host with a real
/// policy answers a caller that policy does not name.
struct AdmitOnly(CommandPrincipal);

impl CommandAuthorizer<CommandEnvelope> for AdmitOnly {
    fn authorize(
        &self,
        principal: &CommandPrincipal,
        _command: &CommandEnvelope,
    ) -> Result<(), AuthorizationDeniedError> {
        if principal == &self.0 {
            return Ok(());
        }
        Err(AuthorizationDeniedError::new(format!("only {} may decide", self.0)))
    }
}

/// One `CreateSchedule` as the `Decide` endpoint's request message.
fn create_schedule(id: &str) -> decider_v1::DecideRequest {
    decider_v1::DecideRequest {
        command: buffa::MessageField::some(buffa_types::google::protobuf::Any {
            type_url: format!("{TYPE_URL_PREFIX}{CREATE_SCHEDULE}"),
            value: create_schedule_command(id).into(),
            ..buffa_types::google::protobuf::Any::default()
        }),
        ..decider_v1::DecideRequest::default()
    }
}

fn create_schedule_command(id: &str) -> Vec<u8> {
    v1::CreateSchedule {
        schedule_id: id.to_owned(),
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
                    subject: "agent.run".to_owned(),
                    ttl: buffa::MessageField::none(),
                    source: buffa::MessageField::none(),
                }
                .into(),
            ),
        }),
        message: buffa::MessageField::some(v1::Message {
            content: buffa::MessageField::some(trogonai_proto::content::v1alpha1::Content {
                content_type: "application/json".to_owned(),
                data: br#"{"kind":"heartbeat"}"#.to_vec(),
            }),
            headers: Vec::new(),
        }),
    }
    .encode_to_vec()
}

fn protobuf_headers() -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(CONTENT_TYPE_HEADER, PROTOBUF_CONTENT_TYPE);
    headers
}

struct RunningHost {
    _server: JetStreamTestServer,
    client: async_nats::Client,
    shutdown: CancellationToken,
    serving: tokio::task::JoinHandle<()>,
}

impl RunningHost {
    async fn start() -> Self {
        Self::start_with(TenantPosition::Absent, |host| host).await
    }

    async fn start_with(tenant: TenantPosition, customize: impl FnOnce(DeciderHost) -> DeciderHost) -> Self {
        let server = JetStreamTestServer::start().await;
        let client = server.client().await;
        let js = server.jetstream().await;
        let modules = module_store();
        let config = config(modules.path(), tenant);

        let host = Arc::new(customize(
            DeciderHost::start(&config, &FileModuleSource::new(modules.path()), js)
                .await
                .expect("the host starts"),
        ));
        let shutdown = CancellationToken::new();
        let serving = tokio::spawn({
            let client = client.clone();
            let shutdown = shutdown.clone();
            async move {
                serve_until(host, client, &config, shutdown)
                    .await
                    .expect("the host serves");
            }
        });

        // The subscription is taken inside the spawned task, so a request
        // published before it exists would be delivered to nobody.
        tokio::time::sleep(Duration::from_millis(200)).await;

        Self {
            _server: server,
            client,
            shutdown,
            serving,
        }
    }

    async fn decide(&self, request: &decider_v1::DecideRequest) -> async_nats::Message {
        self.decide_on(ALICE_SUBJECT, request).await
    }

    async fn decide_on(&self, subject: &str, request: &decider_v1::DecideRequest) -> async_nats::Message {
        self.request_on(subject, request.encode_to_vec(), protobuf_headers())
            .await
    }

    async fn request(&self, payload: Vec<u8>, headers: HeaderMap) -> async_nats::Message {
        self.request_on(ALICE_SUBJECT, payload, headers).await
    }

    async fn request_on(&self, subject: &str, payload: Vec<u8>, headers: HeaderMap) -> async_nats::Message {
        self.try_request_on(subject, payload, headers)
            .await
            .expect("the request is delivered")
    }

    async fn try_request_on(
        &self,
        subject: &str,
        payload: Vec<u8>,
        headers: HeaderMap,
    ) -> Result<async_nats::Message, RequestError> {
        tokio::time::timeout(
            Duration::from_secs(20),
            self.client
                .request_with_headers(subject.to_owned(), headers, payload.into()),
        )
        .await
        .expect("the host replies before the test times out")
    }

    /// The `Nats-Msg-Id` the first appended event actually landed under.
    ///
    /// Read back off JetStream rather than recomputed, because the point of the
    /// assertion is that the id in the reply and the id on the stream are one
    /// value and not two that happen to agree.
    async fn first_stream_event_id(&self) -> String {
        let js = jetstream::new(self.client.clone());
        let message: StreamMessage = js
            .get_stream(EVENTS_STREAM)
            .await
            .expect("the host provisioned the events stream")
            .get_raw_message(1)
            .await
            .expect("the command appended an event");

        message
            .headers
            .get("Nats-Msg-Id")
            .expect("every appended event carries its id")
            .as_str()
            .to_owned()
    }

    async fn stop(self) {
        self.shutdown.cancel();
        self.serving.await.expect("the serve loop exits cleanly");
    }
}

/// The code on the reply's `Nats-Service-Error-Code`, which per ADR#0016 is the
/// only thing that makes a reply an error.
fn error_code(message: &async_nats::Message) -> Option<String> {
    message
        .headers
        .as_ref()
        .and_then(|headers| headers.get(NATS_SERVICE_ERROR_CODE_HEADER))
        .map(|value| value.as_str().to_owned())
}

fn response(message: &async_nats::Message) -> decider_v1::DecideResponse {
    assert_eq!(
        error_code(message),
        None,
        "a reply carrying the error header is a service error, and decoding it as the method response would read one message as another"
    );
    decider_v1::DecideResponse::decode_from_slice(&message.payload).expect("the reply is a DecideResponse")
}

/// The `ErrorInfo.reason` a service error carries, which is what tells apart
/// two fault classes reporting the same canonical code.
fn fault_reason(message: &async_nats::Message) -> String {
    let status = Status::decode_from_slice(&message.payload).expect("an error reply is one complete Status");

    assert_eq!(
        error_code(message).as_deref(),
        Some(status.code.to_string().as_str()),
        "0016 makes the header authoritative, so a body disagreeing with it would answer two callers differently"
    );

    find_detail::<ErrorInfo>(&status)
        .expect("every decider status names its reason")
        .reason
}

fn reached_no_handler(error: &RequestError) -> bool {
    matches!(error.kind(), RequestErrorKind::NoResponders)
}

#[tokio::test]
async fn a_command_is_decided_and_its_events_land_in_jetstream() {
    let host = RunningHost::start().await;

    let reply = host.decide(&create_schedule(SCHEDULE_ID)).await;

    let accepted = response(&reply);
    assert_eq!(accepted.stream_position, 1);

    // The whole event, not its name: a caller warming a cache off its own write
    // decodes the reply rather than waiting for the event to come back around.
    let decided = &accepted.events[0];
    let created: v1::ScheduleCreated = decided
        .event
        .as_option()
        .expect("a decided event always carries its payload")
        .unpack_if(v1::ScheduleCreated::TYPE_URL)
        .expect("the payload decodes as what its type url claims")
        .expect("the type url is the event the module declared");
    assert_eq!(created.schedule_id, SCHEDULE_ID);

    // The reply's id is the stream's `Nats-Msg-Id`, which is what lets a caller
    // that also tails the stream apply this event exactly once.
    assert_eq!(decided.id, host.first_stream_event_id().await);

    host.stop().await;
}

#[tokio::test]
async fn a_command_no_module_claims_answers_unroutable_rather_than_timing_out() {
    let host = RunningHost::start().await;

    let mut request = create_schedule(SCHEDULE_ID);
    request.command = buffa::MessageField::some(buffa_types::google::protobuf::Any {
        type_url: format!("{TYPE_URL_PREFIX}trogonai.nowhere.v1.Nothing"),
        ..buffa_types::google::protobuf::Any::default()
    });

    let reply = host.decide(&request).await;

    assert_eq!(fault_reason(&reply), FaultClass::Unroutable.reason());

    host.stop().await;
}

#[tokio::test]
async fn recreating_a_schedule_conflicts_instead_of_forking_its_history() {
    let host = RunningHost::start().await;

    let first = host.decide(&create_schedule(SCHEDULE_ID)).await;
    assert_eq!(
        response(&first).stream_position,
        1,
        "the conflict this test is about only means something if the first write landed"
    );

    let second = host.decide(&create_schedule(SCHEDULE_ID)).await;

    assert_eq!(
        fault_reason(&second),
        FaultClass::Conflict.reason(),
        "a caller told 'storage' would page an operator over a write it lost fairly"
    );

    host.stop().await;
}

#[tokio::test]
async fn a_request_the_host_cannot_read_is_refused_before_the_guest_runs() {
    let host = RunningHost::start().await;

    let reply = host.request(UNDECODABLE_PAYLOAD.to_vec(), protobuf_headers()).await;

    assert_eq!(fault_reason(&reply), FaultClass::InvalidRequest.reason());

    host.stop().await;
}

#[tokio::test]
async fn a_zero_expected_revision_is_refused_rather_than_read_as_no_stream() {
    let host = RunningHost::start().await;

    let reply = host
        .decide(&create_schedule(SCHEDULE_ID).with_expected_revision(0))
        .await;

    assert_eq!(
        fault_reason(&reply),
        FaultClass::InvalidRequest.reason(),
        "an empty stream is the module's no_stream precondition, not a guard a caller gets to assert"
    );

    host.stop().await;
}

#[tokio::test]
async fn a_subject_without_principal_tokens_reaches_no_handler() {
    let host = RunningHost::start().await;

    for subject in ["decider.DeciderService.Decide", "decider.agent.DeciderService.Decide"] {
        let error = host
            .try_request_on(
                subject,
                create_schedule(SCHEDULE_ID).encode_to_vec(),
                protobuf_headers(),
            )
            .await
            .expect_err("no subscription matches a subject that names nobody");

        assert!(
            reached_no_handler(&error),
            "{subject}: a host that received it would have to refuse it, and a subscription that never matches it cannot get that wrong: {error}"
        );
    }

    host.stop().await;
}

#[tokio::test]
async fn a_subject_naming_no_principal_is_refused_before_the_payload_is_read() {
    let host = RunningHost::start().await;

    for subject in [
        "decider.robot.alice.DeciderService.Decide",
        "decider.agent.a*b.DeciderService.Decide",
    ] {
        let reply = host
            .request_on(subject, UNDECODABLE_PAYLOAD.to_vec(), protobuf_headers())
            .await;

        assert_eq!(
            fault_reason(&reply),
            "PRINCIPAL_MISSING",
            "{subject}: those bytes fail decoding, so any other reason means the payload was read first"
        );
        assert_eq!(error_code(&reply), Some((Code::UNAUTHENTICATED as i32).to_string()));
    }

    host.stop().await;
}

#[tokio::test]
async fn a_principal_the_authorizer_denies_is_refused_whatever_its_headers_claim() {
    let host = RunningHost::start_with(TenantPosition::Absent, |host| host.with_authorizer(AdmitOnly(alice()))).await;

    let mut forged = protobuf_headers();
    forged.insert(FORGED_PRINCIPAL_HEADER, ALICE);
    let denied = host
        .request_on(MALLORY_SUBJECT, create_schedule(SCHEDULE_ID).encode_to_vec(), forged)
        .await;

    assert_eq!(
        fault_reason(&denied),
        "PRINCIPAL_UNAUTHORIZED",
        "a header is something the caller wrote, and only the subject is something the server checked"
    );
    assert_eq!(error_code(&denied), Some((Code::PERMISSION_DENIED as i32).to_string()));

    let admitted = host.decide(&create_schedule(SCHEDULE_ID)).await;
    assert_eq!(
        response(&admitted).stream_position,
        1,
        "a denied command appends nothing, so the admitted one is the stream's first write"
    );

    host.stop().await;
}

#[tokio::test]
async fn a_host_serving_several_tenants_reads_the_principal_after_the_tenant_token() {
    let host = RunningHost::start_with(TenantPosition::Wildcard, |host| {
        host.with_authorizer(AdmitOnly(alice()))
    })
    .await;

    let reply = host
        .decide_on(TENANT_ALICE_SUBJECT, &create_schedule(SCHEDULE_ID))
        .await;
    assert_eq!(response(&reply).stream_position, 1);

    let untenanted = host
        .try_request_on(
            ALICE_SUBJECT,
            create_schedule(SCHEDULE_ID).encode_to_vec(),
            protobuf_headers(),
        )
        .await
        .expect_err("a shared host subscribes only where a tenant token leads");
    assert!(
        reached_no_handler(&untenanted),
        "a subject without the tenant token has no tenant for the host to act under: {untenanted}"
    );

    host.stop().await;
}

#[tokio::test]
async fn a_client_whose_prefix_ends_in_its_principal_reaches_the_host() {
    let host = RunningHost::start().await;

    let prefix = PrincipalPrefix::new(
        endpoint(TenantPosition::Absent).prefix(),
        PrincipalKind::Agent,
        &alice_id(),
    )
    .expect("alice can be carried in a subject");
    let binding = EndpointBinding::new(
        &MicroSubjectPrefix::from_input(&SubjectPrefixInput::new(prefix.as_str())).expect("a micro subject prefix"),
        &ServiceName::from_input(&ServiceNameInput::new(DECIDER_SERVICE_NAME)).expect("a micro service name"),
        MethodName::from_input(&MethodNameInput::new(DECIDE_METHOD_NAME)).expect("a micro method name"),
        DiscoveryMetadata::default(),
    )
    .expect("the binding derives its subject");

    let accepted: decider_v1::DecideResponse = grpc_nats_micro::client::request(
        &host.client,
        &binding,
        ContentType::Protobuf,
        &create_schedule(SCHEDULE_ID),
        Duration::from_secs(20),
    )
    .await
    .expect("the host decides what the client sends");

    assert_eq!(binding.subject().as_str(), ALICE_SUBJECT);
    assert_eq!(accepted.stream_position, 1);

    host.stop().await;
}

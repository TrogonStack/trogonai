use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use buffa::Enumeration as _;
use grpc_nats_micro::{ContentType, ReplyError};
use trogon_decider_runtime::{
    AppendStreamRequest, AppendStreamResponse, ReadStreamRequest, ReadStreamResponse, StreamAppend, StreamRead,
};
use trogon_nats::AdvancedMockNatsClient;
use trogon_peer::{PeerId, TenantId};
use trogon_peer_nats::{PeerAccount, PeerIngress, PeerMethod, PeerRpcAdmission, PeerRpcApplication, PeerTenant};
use trogon_peer_service::{
    ContentBinding, ContentOpen, ContentSeal, MessageText, PeerAccess, PeerApplication, PeerPortError, PeerService,
    ScopedPeerStore,
    test_support::{
        CatalogFixture, ClockFixture, DisposableContentFixture, Fixture, IdFixture, PolicyFixture, PresentationFixture,
        StoreFixture,
    },
};
use trogonai_proto::{google::rpc::Code, nats::micro::v1alpha1::ServiceOptions, peers::v1alpha1 as proto};

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

#[derive(Clone, Copy, PartialEq, Eq)]
enum StoreFailure {
    None,
    Read,
    Append,
    UnknownEvent,
    MalformedEvent,
    InvalidHistory,
    HistoryLimit,
    ResumeRead,
    ResumeMalformed,
    ResumeExpired,
}
#[derive(Debug, thiserror::Error)]
enum InjectedStoreError {
    #[error("private-storage-marker")]
    Unavailable,
    #[error("fixture storage failed")]
    Fixture(#[source] Box<dyn std::error::Error + Send + Sync>),
    #[error("fixture append failed")]
    AppendFixture(#[source] <StoreFixture as StreamAppend<PeerId>>::Error),
}
#[derive(Clone)]
struct FaultStore {
    inner: StoreFixture,
    tenant: TenantId,
    failure: StoreFailure,
    reads: Arc<AtomicUsize>,
    injected: Arc<AtomicUsize>,
    clock: ClockFixture,
}
impl ScopedPeerStore for FaultStore {
    fn tenant(&self) -> &TenantId {
        &self.tenant
    }
}
impl StreamRead<PeerId> for FaultStore {
    type Error = InjectedStoreError;
    async fn read_stream(&self, request: ReadStreamRequest<'_, PeerId>) -> Result<ReadStreamResponse, Self::Error> {
        let call = self.reads.fetch_add(1, Ordering::SeqCst) + 1;
        if matches!(self.failure, StoreFailure::Read) || (matches!(self.failure, StoreFailure::ResumeRead) && call == 3)
        {
            self.injected.fetch_add(1, Ordering::SeqCst);
            return Err(InjectedStoreError::Unavailable);
        }
        let mut read = self
            .inner
            .read_stream(request)
            .await
            .map_err(|e| InjectedStoreError::Fixture(Box::new(e)))?;
        let first = read.events.first().cloned().ok_or(InjectedStoreError::Unavailable)?;
        if matches!(
            self.failure,
            StoreFailure::UnknownEvent
                | StoreFailure::MalformedEvent
                | StoreFailure::InvalidHistory
                | StoreFailure::HistoryLimit
        ) || (matches!(
            self.failure,
            StoreFailure::ResumeMalformed | StoreFailure::ResumeExpired
        ) && call == 3)
        {
            self.injected.fetch_add(1, Ordering::SeqCst);
        }
        match self.failure {
            StoreFailure::UnknownEvent => read.events[0].event.r#type = "example.private/unrecognized".into(),
            StoreFailure::MalformedEvent => read.events[0].event.content = vec![255],
            StoreFailure::InvalidHistory => read.events.push(first),
            StoreFailure::HistoryLimit => read.events = vec![first; 4097],
            StoreFailure::ResumeMalformed if call == 3 => read.events[0].event.content = vec![255],
            StoreFailure::ResumeExpired if call == 3 => {
                let mut time = self.clock.time.lock().map_err(|_| InjectedStoreError::Unavailable)?;
                let advanced = *time + std::time::Duration::from_secs(11);
                *time = advanced;
            }
            StoreFailure::None
            | StoreFailure::Read
            | StoreFailure::Append
            | StoreFailure::ResumeRead
            | StoreFailure::ResumeMalformed
            | StoreFailure::ResumeExpired => {}
        }
        Ok(read)
    }
    async fn read_stream_bounded(
        &self,
        request: ReadStreamRequest<'_, PeerId>,
        _max: u64,
    ) -> Result<ReadStreamResponse, Self::Error> {
        self.read_stream(request).await
    }
}
impl StreamAppend<PeerId> for FaultStore {
    type Error = InjectedStoreError;
    fn classify_append_failure(&self, error: &Self::Error) -> trogon_decider_runtime::AppendFailure {
        match error {
            InjectedStoreError::AppendFixture(error) => self.inner.classify_append_failure(error),
            InjectedStoreError::Unavailable | InjectedStoreError::Fixture(_) => {
                trogon_decider_runtime::AppendFailure::Fatal
            }
        }
    }
    async fn append_stream(
        &self,
        request: AppendStreamRequest<'_, PeerId>,
    ) -> Result<AppendStreamResponse, Self::Error> {
        if matches!(self.failure, StoreFailure::Append) {
            self.injected.fetch_add(1, Ordering::SeqCst);
            return Err(InjectedStoreError::Unavailable);
        }
        self.inner
            .append_stream(request)
            .await
            .map_err(InjectedStoreError::AppendFixture)
    }
}
#[derive(Clone, Copy)]
enum PortFailure {
    Denied,
    Conflict,
    Unavailable,
    Integrity,
    Backend,
}
impl PortFailure {
    fn error(self) -> PeerPortError {
        match self {
            Self::Denied => PeerPortError::Denied,
            Self::Conflict => PeerPortError::Conflict,
            Self::Unavailable => PeerPortError::Unavailable,
            Self::Integrity => PeerPortError::Integrity,
            Self::Backend => PeerPortError::Backend(Box::new(std::io::Error::other("private-port-marker"))),
        }
    }
}
#[derive(Clone)]
struct FaultContent {
    inner: DisposableContentFixture,
    failure: Option<PortFailure>,
}
impl ContentSeal for FaultContent {
    async fn seal(&self, access: &PeerAccess, text: &MessageText) -> Result<ContentBinding, PeerPortError> {
        if let Some(failure) = self.failure {
            return Err(failure.error());
        }
        self.inner.seal(access, text).await
    }
}
impl ContentOpen for FaultContent {
    async fn open(&self, access: &PeerAccess, binding: &ContentBinding) -> Result<Option<MessageText>, PeerPortError> {
        if let Some(failure) = self.failure {
            return Err(failure.error());
        }
        self.inner.open(access, binding).await
    }
}
type Adapter = PeerRpcApplication<
    FaultStore,
    PolicyFixture,
    ClockFixture,
    FaultContent,
    PresentationFixture,
    IdFixture,
    CatalogFixture,
>;
#[derive(Clone)]
struct FaultAdapter {
    backend: Arc<Adapter>,
    failure: StoreFailure,
    injected: Arc<AtomicUsize>,
}
fn adapter(fixture: &Fixture, failure: StoreFailure, port: Option<PortFailure>, tenant: TenantId) -> FaultAdapter {
    let injected = Arc::default();
    let store = FaultStore {
        inner: fixture.store.clone(),
        tenant,
        failure,
        reads: Arc::default(),
        injected: Arc::clone(&injected),
        clock: fixture.clock.clone(),
    };
    let backend = Arc::new(PeerRpcApplication::new(
        PeerService::new(store.clone(), fixture.policy.clone(), fixture.clock.clone()),
        PeerApplication::new(
            store,
            fixture.policy.clone(),
            fixture.clock.clone(),
            FaultContent {
                inner: fixture.content.clone(),
                failure: port,
            },
            fixture.cache.clone(),
            fixture.ids.clone(),
            fixture.catalog.clone(),
        ),
    ));
    FaultAdapter {
        backend,
        failure,
        injected,
    }
}
async fn assert_wire_fault<Request: buffa::Message + serde::Serialize>(
    adapter: FaultAdapter,
    method: PeerMethod,
    request: &Request,
    code: Code,
    message: &str,
) -> TestResult {
    let FaultAdapter {
        backend,
        failure,
        injected,
    } = adapter;
    let admission = PeerRpcAdmission {
        ingress: PeerIngress::new(PeerAccount::SharedFixture {
            tenant: PeerTenant::new("tenant")?,
        }),
        method,
        backend,
    };
    let mut headers = async_nats::HeaderMap::new();
    headers.insert("Content-Type", ContentType::Protobuf.header_value());
    let request = async_nats::Message {
        subject: format!("tenant.peer.v1.alice.PeerService.{}", method.as_str()).into(),
        reply: Some("_INBOX.tenant.alice.connection.fault".into()),
        payload: ContentType::Protobuf.encode(request)?.into(),
        headers: Some(headers),
        status: None,
        description: None,
        length: 0,
    };
    let published = AdvancedMockNatsClient::new();
    grpc_nats_micro::server::dispatch_admitted(&published, &request, &ServiceOptions::default(), &admission).await;
    assert_eq!(published.published_messages(), ["_INBOX.tenant.alice.connection.fault"]);
    let headers = published.published_headers();
    let payloads = published.published_payloads();
    let fault = match grpc_nats_micro::status_codec::decode_reply::<proto::EnrollResponse>(
        Some(&headers[0]),
        &payloads[0],
        ContentType::Protobuf,
    ) {
        Err(ReplyError::Service(error)) => error,
        _ => return Err("expected a canonical service fault".into()),
    };
    assert_eq!(fault.status().code, code.to_i32());
    assert_eq!(fault.message(), message);
    assert!(fault.status().details.is_empty());
    assert!(!String::from_utf8_lossy(&payloads[0]).contains("private-"));
    assert_eq!(
        injected.load(Ordering::SeqCst),
        usize::from(failure != StoreFailure::None),
        "the selected fault must reach its intended boundary exactly once"
    );
    Ok(())
}
fn heartbeat(f: &Fixture) -> proto::HeartbeatRequest {
    proto::HeartbeatRequest {
        attachment_ref: f.a.clone().into(),
        availability: Some(proto::PeerAvailability::PEER_AVAILABILITY_READY.into()),
        ..Default::default()
    }
}
fn status(f: &Fixture) -> proto::StatusRequest {
    proto::StatusRequest {
        attachment_ref: f.a.clone().into(),
        idempotency_key: Some("absent".into()),
    }
}

#[tokio::test]
async fn rpc_history_faults_remain_bounded_and_do_not_disclose_storage_details() -> TestResult {
    for (failure, code, message) in [
        (StoreFailure::Read, Code::UNAVAILABLE, "peer storage unavailable"),
        (StoreFailure::UnknownEvent, Code::INTERNAL, "peer history invalid"),
        (StoreFailure::MalformedEvent, Code::INTERNAL, "peer history invalid"),
        (StoreFailure::InvalidHistory, Code::INTERNAL, "peer history invalid"),
        (
            StoreFailure::HistoryLimit,
            Code::RESOURCE_EXHAUSTED,
            "peer history limit reached",
        ),
    ] {
        let f = Fixture::new().await?;
        let before = f.store.appends.load(Ordering::SeqCst);
        assert_wire_fault(
            adapter(&f, failure, None, f.alice.tenant().clone()),
            PeerMethod::Status,
            &status(&f),
            code,
            message,
        )
        .await?;
        assert_eq!(f.store.appends.load(Ordering::SeqCst), before);
    }
    Ok(())
}

#[tokio::test]
async fn rpc_command_failures_preserve_denial_capacity_storage_and_precondition_categories() -> TestResult {
    for (failure, code, message) in [
        (StoreFailure::Read, Code::UNAVAILABLE, "peer storage unavailable"),
        (StoreFailure::Append, Code::UNAVAILABLE, "peer storage unavailable"),
        (StoreFailure::MalformedEvent, Code::INTERNAL, "peer execution failed"),
        (StoreFailure::InvalidHistory, Code::INTERNAL, "peer execution failed"),
        (
            StoreFailure::HistoryLimit,
            Code::RESOURCE_EXHAUSTED,
            "peer capacity exhausted",
        ),
    ] {
        let f = Fixture::new().await?;
        assert_wire_fault(
            adapter(&f, failure, None, f.alice.tenant().clone()),
            PeerMethod::Heartbeat,
            &heartbeat(&f),
            code,
            message,
        )
        .await?;
    }
    let f = Fixture::new().await?;
    f.policy.deny.store(true, Ordering::SeqCst);
    assert_wire_fault(
        adapter(&f, StoreFailure::None, None, f.alice.tenant().clone()),
        PeerMethod::Heartbeat,
        &heartbeat(&f),
        Code::PERMISSION_DENIED,
        "peer operation denied",
    )
    .await?;
    f.policy.deny.store(false, Ordering::SeqCst);
    let mut fenced = heartbeat(&f);
    let mut reference = f.a.clone();
    reference.generation = Some(2);
    fenced.attachment_ref = reference.into();
    assert_wire_fault(
        adapter(&f, StoreFailure::None, None, f.alice.tenant().clone()),
        PeerMethod::Heartbeat,
        &fenced,
        Code::FAILED_PRECONDITION,
        "peer precondition failed",
    )
    .await?;
    let detach = proto::DetachRequest {
        attachment_ref: fenced.attachment_ref,
    };
    assert_wire_fault(
        adapter(&f, StoreFailure::None, None, f.alice.tenant().clone()),
        PeerMethod::Detach,
        &detach,
        Code::FAILED_PRECONDITION,
        "peer precondition failed",
    )
    .await?;
    let ack = proto::AcknowledgeBatchRequest {
        attachment_ref: f.a.clone().into(),
        batch_id: Some("missing-batch".into()),
    };
    assert_wire_fault(
        adapter(&f, StoreFailure::None, None, f.alice.tenant().clone()),
        PeerMethod::AcknowledgeBatch,
        &ack,
        Code::FAILED_PRECONDITION,
        "peer precondition failed",
    )
    .await?;
    Ok(())
}

#[tokio::test]
async fn rpc_content_port_errors_keep_their_category_and_have_no_durable_send_effect() -> TestResult {
    for (failure, code, message) in [
        (PortFailure::Denied, Code::PERMISSION_DENIED, "peer operation denied"),
        (PortFailure::Conflict, Code::ABORTED, "peer record conflict"),
        (PortFailure::Unavailable, Code::UNAVAILABLE, "peer port unavailable"),
        (PortFailure::Integrity, Code::INTERNAL, "peer integrity check failed"),
        (PortFailure::Backend, Code::UNAVAILABLE, "peer port unavailable"),
    ] {
        let f = Fixture::new().await?;
        let before = f.store.appends.load(Ordering::SeqCst);
        assert_wire_fault(
            adapter(&f, StoreFailure::None, Some(failure), f.alice.tenant().clone()),
            PeerMethod::Send,
            &f.send("fault", "body"),
            code,
            message,
        )
        .await?;
        assert_eq!(f.store.appends.load(Ordering::SeqCst), before);
    }
    Ok(())
}

#[tokio::test]
async fn rpc_input_scope_context_and_evidence_errors_are_sanitized() -> TestResult {
    let f = Fixture::new().await?;
    let backend = adapter(&f, StoreFailure::None, None, f.alice.tenant().clone());
    assert_wire_fault(
        backend.clone(),
        PeerMethod::Send,
        &proto::SendRequest::default(),
        Code::INVALID_ARGUMENT,
        "invalid peer request",
    )
    .await?;
    let mut invalid = f.send("invalid", "body");
    invalid.idempotency_key = Some(String::new());
    assert_wire_fault(
        backend.clone(),
        PeerMethod::Send,
        &invalid,
        Code::INVALID_ARGUMENT,
        "invalid peer request",
    )
    .await?;
    let mut observation = heartbeat(&f);
    observation.availability = Some(proto::PeerAvailability::PEER_AVAILABILITY_UNSPECIFIED.into());
    assert_wire_fault(
        backend.clone(),
        PeerMethod::Heartbeat,
        &observation,
        Code::INVALID_ARGUMENT,
        "invalid peer request",
    )
    .await?;
    assert_wire_fault(
        backend.clone(),
        PeerMethod::Status,
        &status(&f),
        Code::FAILED_PRECONDITION,
        "peer precondition failed",
    )
    .await?;
    assert_wire_fault(
        backend,
        PeerMethod::Send,
        &f.send("frame", &"<".repeat(2048)),
        Code::INTERNAL,
        "peer execution failed",
    )
    .await?;
    let outside = adapter(&f, StoreFailure::None, None, TenantId::new("different")?);
    assert_wire_fault(
        outside.clone(),
        PeerMethod::Heartbeat,
        &heartbeat(&f),
        Code::PERMISSION_DENIED,
        "peer operation denied",
    )
    .await?;
    assert_wire_fault(
        outside,
        PeerMethod::Enroll,
        &Fixture::enrollment("alice"),
        Code::PERMISSION_DENIED,
        "peer operation denied",
    )
    .await?;
    Ok(())
}

#[tokio::test]
async fn rpc_enrollment_rejects_missing_invalid_live_conflict_and_storage_errors() -> TestResult {
    let f = Fixture::new().await?;
    let backend = adapter(&f, StoreFailure::None, None, f.alice.tenant().clone());
    assert_wire_fault(
        backend.clone(),
        PeerMethod::Enroll,
        &proto::EnrollRequest::default(),
        Code::INVALID_ARGUMENT,
        "invalid peer request",
    )
    .await?;
    let mut invalid = Fixture::enrollment("alice");
    invalid.installation_id = Some(String::new());
    assert_wire_fault(
        backend.clone(),
        PeerMethod::Enroll,
        &invalid,
        Code::INVALID_ARGUMENT,
        "invalid peer request",
    )
    .await?;
    let mut conflict = Fixture::enrollment("alice");
    conflict.connector_id = Some("other-connector".into());
    assert_wire_fault(
        backend,
        PeerMethod::Enroll,
        &conflict,
        Code::FAILED_PRECONDITION,
        "peer precondition failed",
    )
    .await?;
    assert_wire_fault(
        adapter(&f, StoreFailure::Read, None, f.alice.tenant().clone()),
        PeerMethod::Enroll,
        &Fixture::enrollment("alice"),
        Code::UNAVAILABLE,
        "peer storage unavailable",
    )
    .await?;
    Ok(())
}

#[tokio::test]
async fn rpc_resume_rechecks_history_and_lease_before_confirming_the_existing_attachment() -> TestResult {
    for (failure, code, message) in [
        (StoreFailure::ResumeRead, Code::UNAVAILABLE, "peer storage unavailable"),
        (StoreFailure::ResumeMalformed, Code::INTERNAL, "peer history invalid"),
        (
            StoreFailure::ResumeExpired,
            Code::FAILED_PRECONDITION,
            "peer attachment changed",
        ),
    ] {
        let f = Fixture::new().await?;
        assert_wire_fault(
            adapter(&f, failure, None, f.alice.tenant().clone()),
            PeerMethod::Enroll,
            &Fixture::enrollment("alice"),
            code,
            message,
        )
        .await?;
    }
    Ok(())
}

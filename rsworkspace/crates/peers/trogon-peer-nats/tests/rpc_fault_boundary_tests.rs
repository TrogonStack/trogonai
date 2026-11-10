use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};

use buffa::Enumeration as _;
use grpc_nats_micro::{ContentType, ReplyError};
use trogon_decider_runtime::{
    AppendStreamRequest, AppendStreamResponse, ReadFrom, ReadStreamRequest, ReadStreamResponse, StreamAppend,
    StreamRead,
};
use trogon_nats::AdvancedMockNatsClient;
use trogon_peer::{EnrollmentIdentity, InstallationId, NativeSessionId, PeerId, TenantId};
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
    ReplayMalformed,
    ReplayInvalidHistory,
    ReplayHistoryLimit,
    ResumeRead,
    ResumeMalformed,
    ResumeExpired,
    ResumeReplayPastSnapshot,
    ResumeConfirmationPastSnapshot,
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
struct CapturedPastRead {
    peer: PeerId,
    from: ReadFrom,
    response: ReadStreamResponse,
    observed: Arc<Mutex<Vec<ReadStreamResponse>>>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
struct ReplayFaultRead {
    peer: PeerId,
    from: ReadFrom,
    maximum: Option<u64>,
    injected: bool,
}

#[derive(Clone)]
struct FaultStore {
    inner: StoreFixture,
    tenant: TenantId,
    failure: StoreFailure,
    reads: Arc<AtomicUsize>,
    injected: Arc<AtomicUsize>,
    clock: ClockFixture,
    past_read: Option<CapturedPastRead>,
    replay_observed: Arc<Mutex<Vec<ReplayFaultRead>>>,
}
impl ScopedPeerStore for FaultStore {
    fn tenant(&self) -> &TenantId {
        &self.tenant
    }
}
impl FaultStore {
    async fn read_with_replay_fault(
        &self,
        request: ReadStreamRequest<'_, PeerId>,
        maximum: Option<u64>,
    ) -> Result<ReadStreamResponse, InjectedStoreError> {
        let ordinal = self.reads.fetch_add(1, Ordering::SeqCst) + 1;
        assert!((1..=2).contains(&ordinal), "the command cannot retry a replay fault");
        assert_eq!(request.from, ReadFrom::Beginning);
        assert_eq!(
            maximum,
            Some(4097),
            "strict history and command replay both retain the4096-event ceiling"
        );
        let receipt = ReplayFaultRead {
            peer: request.stream_id.clone(),
            from: request.from,
            maximum,
            injected: ordinal == 2,
        };
        let mut read = match maximum {
            Some(maximum) => self.inner.read_stream_bounded(request, maximum).await,
            None => self.inner.read_stream(request).await,
        }
        .map_err(|error| InjectedStoreError::Fixture(Box::new(error)))?;
        if ordinal == 1 {
            assert_eq!(self.injected.load(Ordering::SeqCst), 0);
            let mut history = trogon_peer::PeerHistory::initial();
            for envelope in &read.events {
                let decoded = envelope
                    .decode::<proto::PeerEvent>()
                    .map_err(|error| InjectedStoreError::Fixture(Box::new(error)))?
                    .into_decoded()
                    .ok_or(InjectedStoreError::Unavailable)?;
                history = history
                    .evolve(&decoded)
                    .map_err(|error| InjectedStoreError::Fixture(Box::new(error)))?;
            }
        } else {
            let observed = self
                .replay_observed
                .lock()
                .map_err(|_| InjectedStoreError::Unavailable)?;
            assert_eq!(
                observed.len(),
                1,
                "validated strict pre-read must complete before injection"
            );
            assert_eq!(observed[0].peer, receipt.peer);
            assert!(!observed[0].injected);
            drop(observed);
            assert_eq!(self.injected.fetch_add(1, Ordering::SeqCst), 0);
            let first = read.events.first().cloned().ok_or(InjectedStoreError::Unavailable)?;
            match self.failure {
                StoreFailure::ReplayMalformed => read.events[0].event.content = vec![255],
                StoreFailure::ReplayInvalidHistory => read.events.push(first),
                StoreFailure::ReplayHistoryLimit => read.events = vec![first; 4097],
                _ => return Err(InjectedStoreError::Unavailable),
            }
        }
        self.replay_observed
            .lock()
            .map_err(|_| InjectedStoreError::Unavailable)?
            .push(receipt);
        Ok(read)
    }

    async fn read_with_past_snapshot(
        &self,
        request: ReadStreamRequest<'_, PeerId>,
        max: Option<u64>,
    ) -> Result<ReadStreamResponse, InjectedStoreError> {
        let call = self.reads.fetch_add(1, Ordering::SeqCst) + 1;
        let captured = self.past_read.as_ref().ok_or(InjectedStoreError::Unavailable)?;
        assert_eq!(request.stream_id, &captured.peer);
        assert_eq!(request.from, captured.from);
        assert_eq!(captured.from, ReadFrom::Beginning);
        let target = match self.failure {
            StoreFailure::ResumeReplayPastSnapshot => 2,
            StoreFailure::ResumeConfirmationPastSnapshot => 3,
            _ => return Err(InjectedStoreError::Unavailable),
        };
        let read = if call == target {
            assert_eq!(self.injected.fetch_add(1, Ordering::SeqCst), 0);
            if let Some(max) = max {
                assert!(captured.response.events.len() as u64 <= max);
            }
            captured.response.clone()
        } else {
            let started = std::time::SystemTime::now();
            let read = match max {
                Some(max) => self.inner.read_stream_bounded(request, max).await,
                None => self.inner.read_stream(request).await,
            }
            .map_err(|error| InjectedStoreError::Fixture(Box::new(error)))?;
            assert_current_read_timestamps(&read, started, std::time::SystemTime::now());
            read
        };
        captured
            .observed
            .lock()
            .map_err(|_| InjectedStoreError::Unavailable)?
            .push(read.clone());
        Ok(read)
    }
}
impl StreamRead<PeerId> for FaultStore {
    type Error = InjectedStoreError;
    async fn read_stream(&self, request: ReadStreamRequest<'_, PeerId>) -> Result<ReadStreamResponse, Self::Error> {
        if matches!(
            self.failure,
            StoreFailure::ReplayMalformed | StoreFailure::ReplayInvalidHistory | StoreFailure::ReplayHistoryLimit
        ) {
            return self.read_with_replay_fault(request, None).await;
        }
        if matches!(
            self.failure,
            StoreFailure::ResumeReplayPastSnapshot | StoreFailure::ResumeConfirmationPastSnapshot
        ) {
            return self.read_with_past_snapshot(request, None).await;
        }
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
            | StoreFailure::ResumeExpired
            | StoreFailure::ResumeReplayPastSnapshot
            | StoreFailure::ResumeConfirmationPastSnapshot
            | StoreFailure::ReplayMalformed
            | StoreFailure::ReplayInvalidHistory
            | StoreFailure::ReplayHistoryLimit => {}
        }
        Ok(read)
    }
    async fn read_stream_bounded(
        &self,
        request: ReadStreamRequest<'_, PeerId>,
        max: u64,
    ) -> Result<ReadStreamResponse, Self::Error> {
        if matches!(
            self.failure,
            StoreFailure::ReplayMalformed | StoreFailure::ReplayInvalidHistory | StoreFailure::ReplayHistoryLimit
        ) {
            return self.read_with_replay_fault(request, Some(max)).await;
        }
        if matches!(
            self.failure,
            StoreFailure::ResumeReplayPastSnapshot | StoreFailure::ResumeConfirmationPastSnapshot
        ) {
            return self.read_with_past_snapshot(request, Some(max)).await;
        }
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
    replay_observed: Arc<Mutex<Vec<ReplayFaultRead>>>,
    reads: Arc<AtomicUsize>,
}
fn adapter(fixture: &Fixture, failure: StoreFailure, port: Option<PortFailure>, tenant: TenantId) -> FaultAdapter {
    adapter_with_past_read(fixture, failure, port, tenant, None)
}
fn adapter_with_past_read(
    fixture: &Fixture,
    failure: StoreFailure,
    port: Option<PortFailure>,
    tenant: TenantId,
    past_read: Option<CapturedPastRead>,
) -> FaultAdapter {
    let injected = Arc::default();
    let replay_observed = Arc::default();
    let reads = Arc::default();
    let store = FaultStore {
        inner: fixture.store.clone(),
        tenant,
        failure,
        reads: Arc::clone(&reads),
        injected: Arc::clone(&injected),
        clock: fixture.clock.clone(),
        past_read,
        replay_observed: Arc::clone(&replay_observed),
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
        replay_observed,
        reads,
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
        replay_observed,
        reads,
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
    if matches!(
        failure,
        StoreFailure::ReplayMalformed | StoreFailure::ReplayInvalidHistory | StoreFailure::ReplayHistoryLimit
    ) {
        assert_eq!(reads.load(Ordering::SeqCst), 2, "one strict read then one replay read");
        let observed = replay_observed.lock().map_err(|_| "replay receipt unavailable")?;
        assert_eq!(observed.len(), 2);
        assert_eq!(observed[0].peer, observed[1].peer);
        assert_eq!(observed[0].from, ReadFrom::Beginning);
        assert_eq!(observed[1].from, ReadFrom::Beginning);
        assert_eq!(observed[0].maximum, Some(4097));
        assert_eq!(observed[1].maximum, Some(4097));
        assert!(
            !observed[0].injected,
            "strict history was validated before replay fault"
        );
        assert!(observed[1].injected, "only the second read receives the fault");
    }
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

fn assert_current_read_timestamps(
    read: &ReadStreamResponse,
    started: std::time::SystemTime,
    finished: std::time::SystemTime,
) {
    for event in &read.events {
        let timestamp = std::time::SystemTime::from(event.recorded_at);
        assert!(started <= timestamp && timestamp <= finished);
    }
}

// The in-memory fixture stamps each read; injected captures retain their original complete metadata.
fn assert_same_events(
    actual: &[trogon_decider_runtime::StreamEvent],
    expected: &[trogon_decider_runtime::StreamEvent],
) {
    assert_eq!(actual.len(), expected.len());
    for (actual, expected) in actual.iter().zip(expected) {
        assert_eq!(actual.stream_id, expected.stream_id);
        assert_eq!(actual.event, expected.event);
        assert_eq!(actual.stream_position, expected.stream_position);
    }
}

fn assert_same_read_contents(actual: &ReadStreamResponse, expected: &ReadStreamResponse) {
    assert_eq!(actual.current_position, expected.current_position);
    assert_same_events(&actual.events, &expected.events);
}

async fn actual_begin_read(fixture: &Fixture, peer: &PeerId) -> TestResult<ReadStreamResponse> {
    let started = std::time::SystemTime::now();
    let read = fixture
        .store
        .read_stream(ReadStreamRequest {
            stream_id: peer,
            from: ReadFrom::Beginning,
        })
        .await?;
    assert_current_read_timestamps(&read, started, std::time::SystemTime::now());
    Ok(read)
}

async fn real_read_enrollment_resume(
    fixture: &Fixture,
    request: &proto::EnrollRequest,
) -> TestResult<proto::EnrollResponse> {
    let admission = PeerRpcAdmission {
        ingress: PeerIngress::new(PeerAccount::SharedFixture {
            tenant: PeerTenant::new("tenant")?,
        }),
        method: PeerMethod::Enroll,
        backend: adapter(fixture, StoreFailure::None, None, fixture.alice.tenant().clone()).backend,
    };
    let mut headers = async_nats::HeaderMap::new();
    headers.insert("Content-Type", ContentType::Protobuf.header_value());
    let payload = ContentType::Protobuf.encode(request)?;
    let message = async_nats::Message {
        subject: "tenant.peer.v1.alice.PeerService.Enroll".into(),
        reply: Some("_INBOX.tenant.alice.connection.resume".into()),
        length: payload.len(),
        payload: payload.into(),
        headers: Some(headers),
        status: None,
        description: None,
    };
    let client = AdvancedMockNatsClient::new();
    tokio::time::timeout(
        std::time::Duration::from_secs(2),
        grpc_nats_micro::server::dispatch_admitted(&client, &message, &ServiceOptions::default(), &admission),
    )
    .await?;
    assert_eq!(client.published_messages(), ["_INBOX.tenant.alice.connection.resume"]);
    let headers = client.published_headers();
    let payloads = client.published_payloads();
    assert_eq!(headers.len(), 1);
    assert_eq!(payloads.len(), 1);
    assert!(headers[0].get(grpc_nats_micro::constants::HEADER_ERROR_CODE).is_none());
    Ok(grpc_nats_micro::status_codec::decode_reply(
        Some(&headers[0]),
        &payloads[0],
        ContentType::Protobuf,
    )?)
}

#[tokio::test]
async fn rpc_resume_rejects_a_host_replaying_a_genuine_snapshot_older_than_its_preflight() -> TestResult {
    let f = Fixture::new().await?;
    let peer = PeerId::parse(f.a.peer_id.clone().ok_or(PeerPortError::Integrity)?)?;
    let prior = actual_begin_read(&f, &peer).await?;
    let mut request = heartbeat(&f);
    request.availability = Some(proto::PeerAvailability::PEER_AVAILABILITY_BUSY.into());
    let renewed = PeerService::new(f.store.clone(), f.policy.clone(), f.clock.clone())
        .heartbeat(&f.alice, request)
        .await?;
    assert_eq!(renewed.attachment.attachment_ref.as_option(), Some(&f.a));
    let current = actual_begin_read(&f, &peer).await?;
    assert!(
        current
            .current_position
            .zip(prior.current_position)
            .is_some_and(|(current, prior)| current > prior)
    );
    assert_eq!(current.events.len(), prior.events.len() + 1);
    assert_same_events(&current.events[..prior.events.len()], prior.events.as_slice());
    let event = current
        .events
        .last()
        .ok_or(PeerPortError::Integrity)?
        .decode::<proto::PeerEvent>()?
        .into_decoded()
        .ok_or(PeerPortError::Integrity)?;
    assert!(matches!(
        event.event,
        Some(trogonai_proto::peers::PeerEventCase::PeerHeartbeatRecorded(_))
    ));
    let durable_state = f.history(&peer).await?;
    assert_eq!(
        durable_state.availability.and_then(|value| value.as_known()),
        Some(proto::PeerAvailability::PEER_AVAILABILITY_BUSY)
    );
    let observed = Arc::new(Mutex::new(Vec::new()));
    let appends = f.store.appends.load(Ordering::SeqCst);
    let backend = adapter_with_past_read(
        &f,
        StoreFailure::ResumeReplayPastSnapshot,
        None,
        f.alice.tenant().clone(),
        Some(CapturedPastRead {
            peer: peer.clone(),
            from: ReadFrom::Beginning,
            response: prior.clone(),
            observed: Arc::clone(&observed),
        }),
    );
    assert_wire_fault(
        backend,
        PeerMethod::Enroll,
        &Fixture::enrollment("alice"),
        Code::ABORTED,
        "peer revision conflict",
    )
    .await?;
    {
        let reads = observed.lock().map_err(|_| PeerPortError::Unavailable)?;
        assert_eq!(reads.len(), 2);
        assert_same_read_contents(&reads[0], &current);
        assert_eq!(
            reads[1], prior,
            "the injected past snapshot must be returned byte-for-byte"
        );
    }
    assert_eq!(
        f.store.appends.load(Ordering::SeqCst),
        appends + 1,
        "only the rejected create attempt may reach append"
    );
    assert_same_read_contents(&actual_begin_read(&f, &peer).await?, &current);
    assert_eq!(f.history(&peer).await?, durable_state);
    assert_eq!(durable_state.attachment.attachment_ref.as_option(), Some(&f.a));
    let resumed = real_read_enrollment_resume(&f, &Fixture::enrollment("alice")).await?;
    assert_eq!(resumed.attachment, renewed.attachment);
    assert_same_read_contents(&actual_begin_read(&f, &peer).await?, &current);
    Ok(())
}

#[tokio::test]
async fn rpc_resume_rejects_a_host_returning_a_genuine_pre_enrollment_empty_confirmation() -> TestResult {
    let f = Fixture::new().await?;
    let mut request = Fixture::enrollment("alice");
    request.native_session_id = Some("past_read_session".into());
    request.connector_id = Some("past_read_connector".into());
    let identity = EnrollmentIdentity::new(
        f.alice.clone(),
        InstallationId::new(request.installation_id.clone().ok_or(PeerPortError::Integrity)?)?,
        NativeSessionId::new(request.native_session_id.clone().ok_or(PeerPortError::Integrity)?)?,
    );
    let peer = identity.peer_id().clone();
    let empty = actual_begin_read(&f, &peer).await?;
    assert!(empty.events.is_empty());
    assert!(empty.current_position.is_none());
    let enrolled = PeerService::new(f.store.clone(), f.policy.clone(), f.clock.clone())
        .enroll(&f.alice, request.clone())
        .await?;
    assert_eq!(enrolled.peer_id.as_deref(), Some(peer.as_str()));
    let current = actual_begin_read(&f, &peer).await?;
    assert!(current.current_position.is_some());
    assert!(!current.events.is_empty());
    let durable_state = f.history(&peer).await?;
    assert_eq!(
        durable_state.attachment.attachment_ref.as_option(),
        enrolled.attachment.attachment_ref.as_option()
    );
    let observed = Arc::new(Mutex::new(Vec::new()));
    let appends = f.store.appends.load(Ordering::SeqCst);
    assert_wire_fault(
        adapter_with_past_read(
            &f,
            StoreFailure::ResumeConfirmationPastSnapshot,
            None,
            f.alice.tenant().clone(),
            Some(CapturedPastRead {
                peer: peer.clone(),
                from: ReadFrom::Beginning,
                response: empty.clone(),
                observed: Arc::clone(&observed),
            }),
        ),
        PeerMethod::Enroll,
        &request,
        Code::INTERNAL,
        "peer execution failed",
    )
    .await?;
    {
        let reads = observed.lock().map_err(|_| PeerPortError::Unavailable)?;
        assert_eq!(reads.len(), 3);
        assert_same_read_contents(&reads[0], &current);
        assert_same_read_contents(&reads[1], &current);
        assert_eq!(
            reads[2], empty,
            "the injected empty capture must be returned byte-for-byte"
        );
    }
    assert_eq!(
        f.store.appends.load(Ordering::SeqCst),
        appends + 1,
        "only the rejected create attempt may reach append"
    );
    assert_same_read_contents(&actual_begin_read(&f, &peer).await?, &current);
    assert_eq!(f.history(&peer).await?, durable_state);
    let resumed = real_read_enrollment_resume(&f, &request).await?;
    assert_eq!(resumed, enrolled);
    assert_same_read_contents(&actual_begin_read(&f, &peer).await?, &current);
    Ok(())
}

#[tokio::test]
async fn rpc_strict_lifetime_history_keeps_execution_faults_and_query_history_keeps_its_boundary() -> TestResult {
    for (failure, command_code, command_message, query_message) in [
        (
            StoreFailure::Read,
            Code::UNAVAILABLE,
            "peer storage unavailable",
            "peer storage unavailable",
        ),
        (
            StoreFailure::UnknownEvent,
            Code::INTERNAL,
            "peer execution failed",
            "peer history invalid",
        ),
        (
            StoreFailure::MalformedEvent,
            Code::INTERNAL,
            "peer execution failed",
            "peer history invalid",
        ),
        (
            StoreFailure::InvalidHistory,
            Code::INTERNAL,
            "peer execution failed",
            "peer history invalid",
        ),
        (
            StoreFailure::HistoryLimit,
            Code::RESOURCE_EXHAUSTED,
            "peer capacity exhausted",
            "peer history limit reached",
        ),
    ] {
        for method in [
            PeerMethod::Heartbeat,
            PeerMethod::Detach,
            PeerMethod::Status,
            PeerMethod::Roster,
        ] {
            let f = Fixture::new().await?;
            let appends = f.store.appends.load(Ordering::SeqCst);
            let query = matches!(method, PeerMethod::Status | PeerMethod::Roster);
            let (code, message) = if query {
                (command_code, query_message)
            } else {
                (command_code, command_message)
            };
            let rpc = adapter(&f, failure, None, f.alice.tenant().clone());
            match method {
                PeerMethod::Heartbeat => assert_wire_fault(rpc, method, &heartbeat(&f), code, message).await?,
                PeerMethod::Detach => {
                    assert_wire_fault(
                        rpc,
                        method,
                        &proto::DetachRequest {
                            attachment_ref: f.a.clone().into(),
                        },
                        code,
                        message,
                    )
                    .await?
                }
                PeerMethod::Status => assert_wire_fault(rpc, method, &status(&f), code, message).await?,
                PeerMethod::Roster => {
                    assert_wire_fault(
                        rpc,
                        method,
                        &proto::RosterRequest {
                            attachment_ref: f.a.clone().into(),
                            ..Default::default()
                        },
                        code,
                        message,
                    )
                    .await?
                }
                _ => return Err("unexpected lifetime/query boundary method".into()),
            }
            assert_eq!(
                f.store.appends.load(Ordering::SeqCst),
                appends,
                "invalid history cannot append"
            );
        }
    }
    Ok(())
}

#[tokio::test]
async fn rpc_lifetime_replay_faults_preserve_execution_categories_after_validated_history() -> TestResult {
    for (failure, code, message) in [
        (StoreFailure::ReplayMalformed, Code::INTERNAL, "peer execution failed"),
        (
            StoreFailure::ReplayInvalidHistory,
            Code::INTERNAL,
            "peer execution failed",
        ),
        (
            StoreFailure::ReplayHistoryLimit,
            Code::RESOURCE_EXHAUSTED,
            "peer capacity exhausted",
        ),
    ] {
        for method in [PeerMethod::Heartbeat, PeerMethod::Detach] {
            let f = Fixture::new().await?;
            let before = f.store.appends.load(Ordering::SeqCst);
            let rpc = adapter(&f, failure, None, f.alice.tenant().clone());
            match method {
                PeerMethod::Heartbeat => assert_wire_fault(rpc, method, &heartbeat(&f), code, message).await?,
                PeerMethod::Detach => {
                    assert_wire_fault(
                        rpc,
                        method,
                        &proto::DetachRequest {
                            attachment_ref: f.a.clone().into(),
                        },
                        code,
                        message,
                    )
                    .await?
                }
                _ => return Err("unexpected lifecycle replay method".into()),
            }
            assert_eq!(
                f.store.appends.load(Ordering::SeqCst),
                before,
                "failed replay cannot append"
            );
        }
    }
    Ok(())
}

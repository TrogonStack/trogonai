use std::collections::BTreeMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use buffa::Enumeration as _;
use grpc_nats_micro::ServiceFault;
use tracing::{Event, Instrument, Metadata, Subscriber, field::Visit, instrument::WithSubscriber, span};
use trogon_decider_runtime::{
    AppendFailure, AppendStreamRequest, AppendStreamResponse, ReadStreamRequest, ReadStreamResponse, StreamAppend,
    StreamRead,
};
use trogon_peer::{PeerId, TenantId};
use trogon_peer_nats::{PeerRpcApplication, PeerRpcOperation};
use trogon_peer_service::{
    ContentBinding, ContentOpen, ContentSeal, MessageText, PeerAccess, PeerApplication, PeerPortError, PeerService,
    ScopedPeerStore,
    test_support::{
        CatalogFixture, ClockFixture, DisposableContentFixture, Fixture, IdFixture, PolicyFixture, PresentationFixture,
        StoreFixture, TestResult,
    },
};
use trogonai_proto::{google::rpc::Code, peers::v1alpha1 as proto};

const PRIVATE_MARKER: &str = "rpc-sensitive-marker";
const FAULT_TARGET: &str = "trogon_peer_nats::telemetry";

#[derive(Clone, Copy, PartialEq, Eq)]
enum Failure {
    None,
    Read,
    ReplayRead,
    Malformed,
    ReplayMalformed,
    Append,
    Port,
    Context,
}

#[derive(Debug, thiserror::Error)]
enum StoreError {
    #[error("rpc-sensitive-marker")]
    Injected,
    #[error("fixture storage failed")]
    Fixture(#[source] Box<dyn std::error::Error + Send + Sync>),
    #[error("fixture append failed")]
    AppendFixture(#[source] <StoreFixture as StreamAppend<PeerId>>::Error),
}

#[derive(Clone)]
struct FaultStore {
    inner: StoreFixture,
    failure: Failure,
    reads: Arc<AtomicUsize>,
}

impl ScopedPeerStore for FaultStore {
    fn tenant(&self) -> &TenantId {
        self.inner.tenant()
    }
}

impl FaultStore {
    fn fail_read(&self) -> bool {
        let ordinal = self.reads.fetch_add(1, Ordering::SeqCst) + 1;
        self.failure == Failure::Read || (self.failure == Failure::ReplayRead && ordinal == 2)
    }

    fn corrupt_read(&self, mut read: ReadStreamResponse) -> Result<ReadStreamResponse, StoreError> {
        if self.failure == Failure::Malformed
            || (self.failure == Failure::ReplayMalformed && self.reads.load(Ordering::SeqCst) == 2)
        {
            read.events.first_mut().ok_or(StoreError::Injected)?.event.content = vec![255];
        }
        Ok(read)
    }
}

impl StreamRead<PeerId> for FaultStore {
    type Error = StoreError;

    async fn read_stream(&self, request: ReadStreamRequest<'_, PeerId>) -> Result<ReadStreamResponse, Self::Error> {
        if self.fail_read() {
            return Err(StoreError::Injected);
        }
        self.inner
            .read_stream(request)
            .await
            .map_err(|error| StoreError::Fixture(Box::new(error)))
            .and_then(|read| self.corrupt_read(read))
    }

    async fn read_stream_bounded(
        &self,
        request: ReadStreamRequest<'_, PeerId>,
        maximum: u64,
    ) -> Result<ReadStreamResponse, Self::Error> {
        if self.fail_read() {
            return Err(StoreError::Injected);
        }
        self.inner
            .read_stream_bounded(request, maximum)
            .await
            .map_err(|error| StoreError::Fixture(Box::new(error)))
            .and_then(|read| self.corrupt_read(read))
    }
}

impl StreamAppend<PeerId> for FaultStore {
    type Error = StoreError;

    async fn append_stream(
        &self,
        request: AppendStreamRequest<'_, PeerId>,
    ) -> Result<AppendStreamResponse, Self::Error> {
        if self.failure == Failure::Append {
            return Err(StoreError::Injected);
        }
        self.inner
            .append_stream(request)
            .await
            .map_err(StoreError::AppendFixture)
    }

    fn classify_append_failure(&self, error: &Self::Error) -> AppendFailure {
        match error {
            StoreError::AppendFixture(source) => self.inner.classify_append_failure(source),
            StoreError::Injected | StoreError::Fixture(_) => AppendFailure::Fatal,
        }
    }
}

#[derive(Clone)]
struct FaultContent {
    inner: DisposableContentFixture,
    failure: Failure,
}

impl ContentSeal for FaultContent {
    async fn seal(&self, access: &PeerAccess, text: &MessageText) -> Result<ContentBinding, PeerPortError> {
        if self.failure == Failure::Port {
            return Err(PeerPortError::Backend(Box::new(std::io::Error::other(PRIVATE_MARKER))));
        }
        self.inner.seal(access, text).await
    }
}

impl ContentOpen for FaultContent {
    async fn open(&self, access: &PeerAccess, binding: &ContentBinding) -> Result<Option<MessageText>, PeerPortError> {
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

fn adapter(fixture: &Fixture, failure: Failure) -> Adapter {
    let store = FaultStore {
        inner: fixture.store.clone(),
        failure,
        reads: Arc::default(),
    };
    PeerRpcApplication::new(
        PeerService::new(store.clone(), fixture.policy.clone(), fixture.clock.clone()),
        PeerApplication::new(
            store,
            fixture.policy.clone(),
            fixture.clock.clone(),
            FaultContent {
                inner: fixture.content.clone(),
                failure,
            },
            fixture.cache.clone(),
            fixture.ids.clone(),
            fixture.catalog.clone(),
        ),
    )
}

#[derive(Default)]
struct Fields(BTreeMap<String, String>);

impl Visit for Fields {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        self.0.insert(field.name().into(), format!("{value:?}"));
    }

    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        self.0.insert(field.name().into(), value.into());
    }

    fn record_i64(&mut self, field: &tracing::field::Field, value: i64) {
        self.0.insert(field.name().into(), value.to_string());
    }
}

struct RecordedEvent {
    target: String,
    fields: BTreeMap<String, String>,
    contextual: bool,
    inside_span: bool,
}

#[derive(Clone, Default)]
struct Events {
    records: Arc<Mutex<Vec<RecordedEvent>>>,
    entered: Arc<AtomicUsize>,
}

impl Subscriber for Events {
    fn enabled(&self, _: &Metadata<'_>) -> bool {
        true
    }
    fn new_span(&self, _: &span::Attributes<'_>) -> span::Id {
        span::Id::from_u64(1)
    }
    fn record(&self, _: &span::Id, _: &span::Record<'_>) {}
    fn record_follows_from(&self, _: &span::Id, _: &span::Id) {}
    fn enter(&self, _: &span::Id) {
        self.entered.fetch_add(1, Ordering::SeqCst);
    }
    fn exit(&self, _: &span::Id) {
        self.entered.fetch_sub(1, Ordering::SeqCst);
    }
    fn event(&self, event: &Event<'_>) {
        let mut fields = Fields::default();
        event.record(&mut fields);
        if let Ok(mut records) = self.records.lock() {
            records.push(RecordedEvent {
                target: event.metadata().target().into(),
                fields: fields.0,
                contextual: event.is_contextual(),
                inside_span: self.entered.load(Ordering::SeqCst) > 0,
            });
        }
    }
}

fn assert_fault<T>(result: Result<T, ServiceFault>, code: Code, message: &str) -> TestResult {
    let fault = result.err().ok_or("expected service fault")?;
    assert_eq!(fault.status().code, code.to_i32());
    assert_eq!(fault.message(), message);
    assert!(fault.status().details.is_empty());
    assert!(!format!("{:?}", fault.status()).contains(PRIVATE_MARKER));
    Ok(())
}

#[tokio::test]
async fn rpc_faults_record_distinct_bounded_origins_without_disclosing_backend_errors() -> TestResult {
    for (failure, origin, code, message) in [
        (
            Failure::Read,
            "history.read",
            Code::UNAVAILABLE,
            "peer storage unavailable",
        ),
        (
            Failure::ReplayRead,
            "execution.read_stream",
            Code::UNAVAILABLE,
            "peer storage unavailable",
        ),
        (
            Failure::Append,
            "execution.append",
            Code::UNAVAILABLE,
            "peer storage unavailable",
        ),
        (
            Failure::Malformed,
            "history.integrity",
            Code::INTERNAL,
            "peer history invalid",
        ),
        (
            Failure::ReplayMalformed,
            "execution.integrity",
            Code::INTERNAL,
            "peer execution failed",
        ),
        (
            Failure::Port,
            "port.backend",
            Code::UNAVAILABLE,
            "peer port unavailable",
        ),
        (Failure::Context, "context", Code::INTERNAL, "peer execution failed"),
    ] {
        let fixture = Fixture::new().await?;
        let adapter = adapter(&fixture, failure);
        let events = Events::default();
        let text = if failure == Failure::Context {
            "<".repeat(2048)
        } else {
            "message".into()
        };
        let span = tracing::subscriber::with_default(events.clone(), || tracing::info_span!("rpc_fault_contract"));
        async {
            match failure {
                Failure::Read | Failure::Malformed => assert_fault(
                    adapter
                        .execute(
                            &fixture.alice,
                            proto::RosterRequest {
                                attachment_ref: fixture.a.clone().into(),
                                ..Default::default()
                            },
                        )
                        .await,
                    code,
                    message,
                )?,
                Failure::ReplayRead | Failure::ReplayMalformed | Failure::Append => assert_fault(
                    adapter
                        .execute(
                            &fixture.alice,
                            proto::HeartbeatRequest {
                                attachment_ref: fixture.a.clone().into(),
                                availability: Some(proto::PeerAvailability::PEER_AVAILABILITY_READY.into()),
                                ..Default::default()
                            },
                        )
                        .await,
                    code,
                    message,
                )?,
                Failure::Port | Failure::Context => assert_fault(
                    adapter.execute(&fixture.alice, fixture.send("telemetry", &text)).await,
                    code,
                    message,
                )?,
                Failure::None => return Err("unexpected success case".into()),
            }
            Ok::<_, Box<dyn std::error::Error + Send + Sync>>(())
        }
        .instrument(span)
        .with_subscriber(events.clone())
        .await?;
        assert_event(&events, origin, code)?;
    }
    Ok(())
}

fn assert_event(events: &Events, origin: &str, code: Code) -> TestResult {
    let records = events.records.lock().map_err(|_| "telemetry collector poisoned")?;
    assert!(
        records
            .iter()
            .all(|event| event.fields.values().all(|value| !value.contains(PRIVATE_MARKER)))
    );
    let faults = records
        .iter()
        .filter(|event| event.target == FAULT_TARGET)
        .collect::<Vec<_>>();
    assert_eq!(
        faults.len(),
        1,
        "each public RPC failure must preserve its server-side origin"
    );
    assert_eq!(
        faults[0].fields,
        BTreeMap::from([
            ("error.type".into(), origin.into()),
            ("rpc.grpc.status_code".into(), code.to_i32().to_string()),
            ("message".into(), "peer RPC fault".into()),
        ])
    );
    assert!(faults[0].contextual && faults[0].inside_span);
    Ok(())
}

#[tokio::test]
async fn successful_rpc_does_not_emit_a_fault_event() -> TestResult {
    let fixture = Fixture::new().await?;
    let adapter = adapter(&fixture, Failure::None);
    let events = Events::default();
    let _: proto::RosterResponse = adapter
        .execute(
            &fixture.alice,
            proto::RosterRequest {
                attachment_ref: fixture.a.clone().into(),
                ..Default::default()
            },
        )
        .with_subscriber(events.clone())
        .await
        .map_err(|_| "successful roster failed")?;
    let records = events.records.lock().map_err(|_| "telemetry collector poisoned")?;
    assert!(records.iter().all(|event| event.target != FAULT_TARGET));
    Ok(())
}

#[tokio::test]
async fn enrollment_and_lifetime_faults_preserve_their_server_side_origins() -> TestResult {
    for (failure, origin) in [
        (Failure::Read, "history.read"),
        (Failure::ReplayRead, "execution.read_stream"),
        (Failure::Append, "execution.append"),
    ] {
        let fixture = Fixture::new().await?;
        let adapter = adapter(&fixture, failure);
        let events = Events::default();
        let span = tracing::subscriber::with_default(events.clone(), || tracing::info_span!("enroll_contract"));
        assert_fault(
            adapter
                .execute(&fixture.alice, Fixture::enrollment("alice"))
                .instrument(span)
                .with_subscriber(events.clone())
                .await,
            Code::UNAVAILABLE,
            "peer storage unavailable",
        )
        .map_err(|error| format!("enrollment {origin}: {error}"))?;
        assert_event(&events, origin, Code::UNAVAILABLE)?;
    }
    for (failure, code, message) in [
        (Failure::Read, Code::UNAVAILABLE, "peer storage unavailable"),
        (Failure::Malformed, Code::INTERNAL, "peer execution failed"),
    ] {
        let fixture = Fixture::new().await?;
        let adapter = adapter(&fixture, failure);
        let events = Events::default();
        let span = tracing::subscriber::with_default(events.clone(), || tracing::info_span!("lifetime_contract"));
        assert_fault(
            adapter
                .execute(
                    &fixture.alice,
                    proto::DetachRequest {
                        attachment_ref: fixture.a.clone().into(),
                    },
                )
                .instrument(span)
                .with_subscriber(events.clone())
                .await,
            code,
            message,
        )
        .map_err(|error| format!("detach preflight: {error}"))?;
        let origin = if failure == Failure::Read {
            "history.read"
        } else {
            "history.integrity"
        };
        assert_event(&events, origin, code)?;
    }
    Ok(())
}

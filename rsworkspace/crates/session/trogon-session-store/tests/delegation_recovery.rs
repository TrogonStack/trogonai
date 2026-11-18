#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use std::{convert::Infallible, sync::{Arc, atomic::{AtomicUsize, Ordering}}, time::{Duration, Instant}};

use async_nats::jetstream;
use buffa::{Message, MessageName};
use sha2::{Digest as _, Sha256};
use trogon_decider_nats::{JetStreamStore, StreamSubject, StreamSubjectResolver, SubjectState, subject_current_position};
use trogon_decider_runtime::{AppendStreamRequest, CommandId, Event, EventId, Headers, ReadFrom, ReadStreamRequest, StreamAppend, StreamRead, StreamWritePrecondition};
use trogon_identity_types::SubjectPrincipal;
use trogon_nats::{NatsToken, test_support::JetStreamTestServer};
use trogon_session_store::{ChildAdmission, CreateAdmission, DispatchAdmission, SessionAdmission, SessionAdmissionLease, SessionFactCodec, SessionKey, SessionRuntime, wire};
use uuid::Uuid;

#[derive(Debug, thiserror::Error)]
#[error("fixture fact binding mismatch")]
struct FixtureBindingError;

struct FixtureCodec;

impl FixtureCodec {
    fn binding(key: &SessionKey, event_id: EventId, event_type: &str) -> Vec<u8> {
        format!("{key}\0{event_id}\0{event_type}\0").into_bytes()
    }
}

impl SessionFactCodec for FixtureCodec {
    type Error = FixtureBindingError;

    async fn seal(&self, key: &SessionKey, event_id: EventId, event_type: &str, original_plaintext: &[u8]) -> Result<Vec<u8>, Self::Error> {
        let mut envelope = Self::binding(key, event_id, event_type);
        envelope.extend_from_slice(original_plaintext);
        Ok(envelope)
    }

    async fn open(&self, key: &SessionKey, event_id: EventId, event_type: &str, envelope: &[u8]) -> Result<Vec<u8>, Self::Error> {
        envelope.strip_prefix(Self::binding(key, event_id, event_type).as_slice()).map(ToOwned::to_owned).ok_or(FixtureBindingError)
    }
}

#[derive(Default)]
struct FixtureAdmission {
    child_checks: AtomicUsize,
}

struct FixtureLease(Instant);

impl FixtureLease {
    fn new() -> Self {
        Self(Instant::now() + Duration::from_secs(30))
    }
}

impl SessionAdmissionLease for FixtureLease {
    type Error = Infallible;

    fn require_live(&self) -> Result<(), Self::Error> { Ok(()) }
    fn deadline(&self) -> Instant { self.0 }
    async fn withdrawn(&self) { std::future::pending().await }
}

impl SessionAdmission for FixtureAdmission {
    type Error = Infallible;
    type Lease = FixtureLease;

    async fn authorize_source(&self, _caller: &SubjectPrincipal, _key: &SessionKey) -> Result<Self::Lease, Self::Error> {
        Ok(FixtureLease::new())
    }

    async fn authorize_create(&self, _material: CreateAdmission<'_>) -> Result<Self::Lease, Self::Error> {
        Ok(FixtureLease::new())
    }

    async fn authorize_dispatch(&self, _material: DispatchAdmission<'_>) -> Result<Self::Lease, Self::Error> {
        Ok(FixtureLease::new())
    }

    async fn authorize_child(&self, material: ChildAdmission<'_>) -> Result<Self::Lease, Self::Error> {
        assert_eq!(material.parent.session_id, "parent");
        assert_eq!(material.dispatch.child_session_id, "child");
        assert_eq!(material.creation.session_id, "child");
        self.child_checks.fetch_add(1, Ordering::SeqCst);
        Ok(FixtureLease::new())
    }
}

#[derive(Debug, thiserror::Error)]
enum FixtureStoreError {
    #[error(transparent)]
    Subject(#[from] async_nats::SubjectError),
    #[error(transparent)]
    Read(#[from] trogon_decider_nats::StreamStoreError),
}

#[derive(Clone)]
struct FixtureResolver;

impl StreamSubjectResolver<SessionKey> for FixtureResolver {
    type Error = FixtureStoreError;

    async fn resolve_subject_state(&self, stream: &jetstream::stream::Stream, key: &SessionKey) -> Result<SubjectState, Self::Error> {
        let subject = StreamSubject::new(key.subject())?;
        let current_position = subject_current_position(stream, &subject).await?;
        Ok(SubjectState { subject, current_position })
    }
}

struct Harness {
    _server: JetStreamTestServer,
    store: Arc<JetStreamStore<FixtureResolver>>,
    admission: Arc<FixtureAdmission>,
    codec: Arc<FixtureCodec>,
}

impl Harness {
    async fn new() -> Self {
        let server = match std::env::var("TROGON_SESSION_TEST_NETWORK") {
            Ok(network) => JetStreamTestServer::start_on_network(&network).await,
            Err(_) => JetStreamTestServer::start().await,
        };
        let js = server.jetstream().await;
        let stream = js.create_stream(jetstream::stream::Config {
            name: "SESSION_RECOVERY".to_owned(),
            subjects: vec!["fixture.session.sessions.>".to_owned()],
            allow_atomic_publish: true,
            storage: jetstream::stream::StorageType::File,
            retention: jetstream::stream::RetentionPolicy::Limits,
            max_messages: 1024,
            max_bytes: 4 * 1024 * 1024,
            discard: jetstream::stream::DiscardPolicy::New,
            deny_delete: true,
            deny_purge: true,
            ..Default::default()
        }).await.expect("create Session stream");
        let snapshots = js.create_key_value(jetstream::kv::Config {
            bucket: "SESSION_RECOVERY_SNAPSHOTS".to_owned(),
            ..Default::default()
        }).await.expect("create unused snapshots bucket");
        let store = Arc::new(JetStreamStore::builder(js, stream, snapshots).with_subject_resolver(FixtureResolver));
        Self { _server: server, store, admission: Arc::new(FixtureAdmission::default()), codec: Arc::new(FixtureCodec) }
    }

    fn runtime(&self) -> SessionRuntime<JetStreamStore<FixtureResolver>, FixtureAdmission, FixtureCodec> {
        SessionRuntime::new(Arc::clone(&self.store), Arc::clone(&self.admission), Arc::clone(&self.codec))
    }

    async fn fact<T: Message + MessageName>(&self, key: &SessionKey, command_id: CommandId, index: usize, value: &T) -> Event {
        let id = command_id.event_id(index);
        Event { id, r#type: T::FULL_NAME.to_owned(), content: self.codec.seal(key, id, T::FULL_NAME, &value.encode_to_vec()).await.unwrap(), headers: Headers::empty() }
    }
}

fn key(id: &str) -> SessionKey {
    SessionKey::new(NatsToken::new("fixture").unwrap(), id).unwrap()
}

fn caller() -> SubjectPrincipal {
    SubjectPrincipal::from_tokens("service", "reconciler").unwrap()
}

fn command_id() -> CommandId {
    CommandId::new(Uuid::new_v4())
}

fn plan(bytes: &[u8]) -> wire::StoredSessionExecutionPlan {
    wire::StoredSessionExecutionPlan {
        plan_bytes: bytes.to_vec(),
        plan_digest: wire::Digest { algorithm: "sha256".to_owned(), value: Sha256::digest(bytes).to_vec() }.into(),
        ..Default::default()
    }
}

fn checkout() -> wire::CheckoutRef {
    wire::CheckoutRef { checkout_id: "checkout".to_owned(), uri: "git://example.test/repo".to_owned(), revision: Some("immutable-revision".to_owned()) }
}

fn started(id: &str, bytes: &[u8]) -> wire::SessionStarted {
    wire::SessionStarted { session_id: id.to_owned(), execution_plan: plan(bytes).into(), checkout_ref: checkout().into(), ..Default::default() }
}

fn dispatch(parent: &wire::SessionStarted) -> wire::DelegationDispatched {
    wire::DelegationDispatched {
        session_id: "parent".to_owned(),
        operation_id: "delegate-operation".to_owned(),
        child_session_id: "child".to_owned(),
        cascade_policy: wire::CascadePolicy::CASCADE_POLICY_CASCADE_ON_PARENT_TERMINAL.into(),
        turn_id: "turn".to_owned(),
        delegate_binding: wire::DelegationBindingRef { binding_name: Some("reviewer".to_owned()), parent_execution_plan_digest: parent.execution_plan.plan_digest.clone() }.into(),
        child_creation: wire::ChildSessionCreation { execution_plan: plan(b"original child plan").into(), checkout_ref: checkout().into(), ..Default::default() }.into(),
    }
}

#[tokio::test]
async fn acknowledged_parent_dispatch_is_recovered_after_runtime_restart_without_reselection() {
    let harness = Harness::new().await;
    let parent_key = key("parent");
    let child_key = key("child");
    let parent = started("parent", b"original parent plan");
    let dispatch = dispatch(&parent);
    let parent_create = command_id();
    let parent_dispatch = command_id();
    let created = harness.store.append_stream(AppendStreamRequest {
        stream_id: &parent_key,
        stream_write_precondition: StreamWritePrecondition::NoStream,
        events: vec![harness.fact(&parent_key, parent_create, 0, &parent).await],
    }).await.unwrap();
    harness.store.append_stream(AppendStreamRequest {
        stream_id: &parent_key,
        stream_write_precondition: StreamWritePrecondition::At(created.stream_position),
        events: vec![harness.fact(&parent_key, parent_dispatch, 0, &dispatch).await],
    }).await.expect("parent dispatch acknowledged before crash");
    assert!(harness.store.read_stream(ReadStreamRequest { stream_id: &child_key, from: ReadFrom::Beginning }).await.unwrap().events.is_empty());

    let restarted = harness.runtime();
    let recovered = restarted.recover_child(&caller(), &parent_key, "delegate-operation").await.expect("recovery creates the exact retained child after restart");
    assert!(recovered.inserted);
    assert_eq!(recovered.child_key, child_key);
    let child = harness.store.read_stream(ReadStreamRequest { stream_id: &child_key, from: ReadFrom::Beginning }).await.unwrap();
    assert_eq!(child.events.len(), 2);
    assert_eq!(child.events[0].event.r#type, wire::SessionStarted::FULL_NAME);
    assert_eq!(child.events[1].event.r#type, wire::ParentLinked::FULL_NAME);
    let original_started = harness.codec.open(&child_key, child.events[0].event.id, &child.events[0].event.r#type, &child.events[0].event.content).await.unwrap();
    assert_eq!(original_started, started("child", b"original child plan").encode_to_vec());
    let original_link = harness.codec.open(&child_key, child.events[1].event.id, &child.events[1].event.r#type, &child.events[1].event.content).await.unwrap();
    let link = wire::ParentLinked::decode_from_slice(&original_link).unwrap();
    assert_eq!(link.parent_dispatched_at.value, 2);
    assert_eq!(link.delegate_binding, dispatch.delegate_binding);
    assert_eq!(link.operation_id, dispatch.operation_id);
    assert_eq!(harness.admission.child_checks.load(Ordering::SeqCst), 1);

    let retried = harness.runtime().recover_child(&caller(), &parent_key, "delegate-operation").await.unwrap();
    assert!(!retried.inserted);
    assert_eq!(retried.creation_position, recovered.creation_position);
    assert_eq!(harness.store.read_stream(ReadStreamRequest { stream_id: &child_key, from: ReadFrom::Beginning }).await.unwrap().events, child.events);
}

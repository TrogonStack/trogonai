#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

use async_nats::jetstream;
use buffa::{Message, MessageName};
use sha2::{Digest as _, Sha256};
use tokio::sync::{Notify, watch};
use trogon_decider_nats::JetStreamStore;
use trogon_decider_runtime::{
    AppendStreamRequest, CommandId, Event, EventId, Headers, ReadFrom, ReadStreamRequest, StreamAppend, StreamRead,
    StreamWritePrecondition,
};
use trogon_identity_types::SubjectPrincipal;
use trogon_nats::{NatsToken, test_support::JetStreamTestServer};
use trogon_session_store::{
    ChildAdmission, CreateAdmission, DispatchAdmission, SessionAdmission, SessionAdmissionLease, SessionFactCodec,
    SessionKey, SessionRuntime, wire,
};
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

    async fn seal(
        &self,
        key: &SessionKey,
        event_id: EventId,
        event_type: &str,
        original_plaintext: &[u8],
    ) -> Result<Vec<u8>, Self::Error> {
        let mut envelope = Self::binding(key, event_id, event_type);
        envelope.extend_from_slice(Uuid::new_v4().as_bytes());
        envelope.extend_from_slice(original_plaintext);
        Ok(envelope)
    }

    async fn open(
        &self,
        key: &SessionKey,
        event_id: EventId,
        event_type: &str,
        envelope: &[u8],
    ) -> Result<Vec<u8>, Self::Error> {
        envelope
            .strip_prefix(Self::binding(key, event_id, event_type).as_slice())
            .and_then(|value| value.get(16..))
            .map(ToOwned::to_owned)
            .ok_or(FixtureBindingError)
    }
}

struct FixtureAdmission {
    child_checks: AtomicUsize,
    deny_child: AtomicBool,
    expired_source: AtomicBool,
    block_child: AtomicBool,
    child_entered: Notify,
    withdrawn: watch::Sender<bool>,
}

impl Default for FixtureAdmission {
    fn default() -> Self {
        Self {
            child_checks: AtomicUsize::new(0),
            deny_child: AtomicBool::new(false),
            expired_source: AtomicBool::new(false),
            block_child: AtomicBool::new(false),
            child_entered: Notify::new(),
            withdrawn: watch::channel(false).0,
        }
    }
}

#[derive(Debug, thiserror::Error)]
#[error("fixture policy refused")]
struct FixturePolicyError;

struct FixtureLease {
    deadline: Instant,
    withdrawn: watch::Receiver<bool>,
}

impl FixtureLease {
    fn new(authority: &FixtureAdmission, expired: bool) -> Self {
        Self {
            deadline: if expired {
                Instant::now() - Duration::from_secs(1)
            } else {
                Instant::now() + Duration::from_secs(30)
            },
            withdrawn: authority.withdrawn.subscribe(),
        }
    }
}

impl SessionAdmissionLease for FixtureLease {
    type Error = FixturePolicyError;

    fn require_live(&self) -> Result<(), Self::Error> {
        if *self.withdrawn.borrow() {
            Err(FixturePolicyError)
        } else {
            Ok(())
        }
    }
    fn deadline(&self) -> Instant {
        self.deadline
    }
    async fn withdrawn(&self) {
        let mut receiver = self.withdrawn.clone();
        loop {
            if *receiver.borrow_and_update() {
                return;
            }
            if receiver.changed().await.is_err() {
                return;
            }
        }
    }
}

impl SessionAdmission for FixtureAdmission {
    type Error = FixturePolicyError;
    type Lease = FixtureLease;

    async fn authorize_source(
        &self,
        _caller: &SubjectPrincipal,
        _key: &SessionKey,
    ) -> Result<Self::Lease, Self::Error> {
        Ok(FixtureLease::new(self, self.expired_source.load(Ordering::SeqCst)))
    }

    async fn authorize_create(&self, _material: CreateAdmission<'_>) -> Result<Self::Lease, Self::Error> {
        Ok(FixtureLease::new(self, false))
    }

    async fn authorize_dispatch(&self, _material: DispatchAdmission<'_>) -> Result<Self::Lease, Self::Error> {
        Ok(FixtureLease::new(self, false))
    }

    async fn authorize_child(&self, material: ChildAdmission<'_>) -> Result<Self::Lease, Self::Error> {
        assert_eq!(material.parent.session_id, "parent");
        assert_eq!(material.dispatch.child_session_id, "child");
        assert_eq!(material.creation.session_id, "child");
        self.child_checks.fetch_add(1, Ordering::SeqCst);
        if self.deny_child.load(Ordering::SeqCst) {
            return Err(FixturePolicyError);
        }
        if self.block_child.load(Ordering::SeqCst) {
            self.child_entered.notify_one();
            std::future::pending::<()>().await;
        }
        Ok(FixtureLease::new(self, false))
    }
}

struct Harness {
    _server: JetStreamTestServer,
    store: Arc<JetStreamStore<trogon_session_store::SessionSubjectResolver>>,
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
        let stream = js
            .create_stream(jetstream::stream::Config {
                name: "SESSION_RECOVERY".to_owned(),
                subjects: vec!["fixture.session.sessions.>".to_owned()],
                allow_atomic_publish: true,
                storage: jetstream::stream::StorageType::File,
                retention: jetstream::stream::RetentionPolicy::Limits,
                max_messages: 1024,
                max_message_size: 262144,
                max_bytes: 4 * 1024 * 1024,
                discard: jetstream::stream::DiscardPolicy::New,
                deny_delete: true,
                deny_purge: true,
                ..Default::default()
            })
            .await
            .expect("create Session stream");
        let snapshots = js
            .create_key_value(jetstream::kv::Config {
                bucket: "SESSION_RECOVERY_SNAPSHOTS".to_owned(),
                ..Default::default()
            })
            .await
            .expect("create unused snapshots bucket");
        let store = Arc::new(
            JetStreamStore::builder(js, stream, snapshots).with_subject_resolver(
                trogon_session_store::SessionSubjectResolver::new(
                    trogon_session_store::SessionStreamProfile::new(
                        "SESSION_RECOVERY",
                        NatsToken::new("fixture").unwrap(),
                        1,
                    )
                    .unwrap(),
                ),
            ),
        );
        Self {
            _server: server,
            store,
            admission: Arc::new(FixtureAdmission::default()),
            codec: Arc::new(FixtureCodec),
        }
    }

    fn runtime(
        &self,
    ) -> SessionRuntime<JetStreamStore<trogon_session_store::SessionSubjectResolver>, FixtureAdmission, FixtureCodec>
    {
        SessionRuntime::new(
            Arc::clone(&self.store),
            Arc::clone(&self.admission),
            Arc::clone(&self.codec),
        )
    }

    async fn fact<T: Message + MessageName>(
        &self,
        key: &SessionKey,
        command_id: CommandId,
        index: usize,
        value: &T,
    ) -> Event {
        let id = command_id.event_id(index);
        Event {
            id,
            r#type: T::FULL_NAME.to_owned(),
            content: self
                .codec
                .seal(key, id, T::FULL_NAME, &value.encode_to_vec())
                .await
                .unwrap(),
            headers: Headers::empty(),
        }
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
        plan_digest: wire::Digest {
            algorithm: "sha256".to_owned(),
            value: Sha256::digest(bytes).to_vec(),
        }
        .into(),
    }
}

fn checkout() -> wire::CheckoutRef {
    wire::CheckoutRef {
        checkout_id: "checkout".to_owned(),
        uri: "git://example.test/repo".to_owned(),
        revision: Some("immutable-revision".to_owned()),
    }
}

fn started(id: &str, bytes: &[u8]) -> wire::SessionStarted {
    wire::SessionStarted {
        session_id: id.to_owned(),
        execution_plan: plan(bytes).into(),
        checkout_ref: checkout().into(),
        ..Default::default()
    }
}

fn dispatch(parent: &wire::SessionStarted) -> wire::DelegationDispatched {
    wire::DelegationDispatched {
        session_id: "parent".to_owned(),
        operation_id: "delegate-operation".to_owned(),
        child_session_id: "child".to_owned(),
        cascade_policy: wire::CascadePolicy::CASCADE_POLICY_CASCADE_ON_PARENT_TERMINAL.into(),
        turn_id: "turn".to_owned(),
        delegate_binding: wire::DelegationBindingRef {
            binding_name: Some("reviewer".to_owned()),
            parent_execution_plan_digest: parent.execution_plan.plan_digest.clone(),
        }
        .into(),
        child_creation: wire::ChildSessionCreation {
            execution_plan: plan(b"original child plan").into(),
            checkout_ref: checkout().into(),
            ..Default::default()
        }
        .into(),
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
    let created = harness
        .store
        .append_stream(AppendStreamRequest {
            stream_id: &parent_key,
            stream_write_precondition: StreamWritePrecondition::NoStream,
            events: vec![harness.fact(&parent_key, parent_create, 0, &parent).await],
        })
        .await
        .unwrap();
    harness
        .store
        .append_stream(AppendStreamRequest {
            stream_id: &parent_key,
            stream_write_precondition: StreamWritePrecondition::At(created.stream_position),
            events: vec![harness.fact(&parent_key, parent_dispatch, 0, &dispatch).await],
        })
        .await
        .expect("parent dispatch acknowledged before crash");
    assert!(
        harness
            .store
            .read_stream(ReadStreamRequest {
                stream_id: &child_key,
                from: ReadFrom::Beginning
            })
            .await
            .unwrap()
            .events
            .is_empty()
    );

    let restarted = harness.runtime();
    let recovered = restarted
        .recover_child(&caller(), &parent_key, "delegate-operation")
        .await
        .expect("recovery creates the exact retained child after restart");
    assert!(recovered.inserted);
    assert_eq!(recovered.child_key, child_key);
    let child = harness
        .store
        .read_stream(ReadStreamRequest {
            stream_id: &child_key,
            from: ReadFrom::Beginning,
        })
        .await
        .unwrap();
    assert_eq!(child.events.len(), 2);
    assert_eq!(child.events[0].event.r#type, wire::SessionStarted::FULL_NAME);
    assert_eq!(child.events[1].event.r#type, wire::ParentLinked::FULL_NAME);
    let original_started = harness
        .codec
        .open(
            &child_key,
            child.events[0].event.id,
            &child.events[0].event.r#type,
            &child.events[0].event.content,
        )
        .await
        .unwrap();
    assert_eq!(
        original_started,
        started("child", b"original child plan").encode_to_vec()
    );
    let original_link = harness
        .codec
        .open(
            &child_key,
            child.events[1].event.id,
            &child.events[1].event.r#type,
            &child.events[1].event.content,
        )
        .await
        .unwrap();
    let link = wire::ParentLinked::decode_from_slice(&original_link).unwrap();
    assert_eq!(link.parent_dispatched_at.value, 2);
    assert_eq!(link.delegate_binding, dispatch.delegate_binding);
    assert_eq!(link.operation_id, dispatch.operation_id);
    assert_eq!(harness.admission.child_checks.load(Ordering::SeqCst), 1);

    let retried = harness
        .runtime()
        .recover_child(&caller(), &parent_key, "delegate-operation")
        .await
        .unwrap();
    assert!(!retried.inserted);
    assert_eq!(retried.creation_position, recovered.creation_position);
    assert_eq!(
        harness
            .store
            .read_stream(ReadStreamRequest {
                stream_id: &child_key,
                from: ReadFrom::Beginning
            })
            .await
            .unwrap()
            .events,
        child.events
    );
}

impl Harness {
    async fn seed_parent(&self) -> wire::DelegationDispatched {
        let parent_key = key("parent");
        let parent = started("parent", b"original parent plan");
        let dispatch = dispatch(&parent);
        self.store
            .append_stream(AppendStreamRequest {
                stream_id: &parent_key,
                stream_write_precondition: StreamWritePrecondition::NoStream,
                events: vec![
                    self.fact(&parent_key, command_id(), 0, &parent).await,
                    self.fact(&parent_key, command_id(), 0, &dispatch).await,
                ],
            })
            .await
            .unwrap();
        dispatch
    }

    async fn child_events(&self) -> Vec<trogon_decider_runtime::StreamEvent> {
        self.store
            .read_stream(ReadStreamRequest {
                stream_id: &key("child"),
                from: ReadFrom::Beginning,
            })
            .await
            .unwrap()
            .events
    }
}

#[tokio::test]
async fn public_create_and_dispatch_keep_originals_then_recovery_runs_once() {
    let harness = Harness::new().await;
    let parent_key = key("parent");
    let parent = started("parent", b"original parent plan");
    let create = wire::CreateSession {
        session_id: parent.session_id.clone(),
        execution_plan: parent.execution_plan.clone(),
        checkout_ref: parent.checkout_ref.clone(),
        candidate_ref: parent.candidate_ref.clone(),
    };
    let create_id = command_id();
    let runtime = harness.runtime();
    let original_position = runtime
        .create_session(&caller(), &parent_key, create_id, &create.encode_to_vec())
        .await
        .unwrap();
    assert_eq!(
        runtime
            .create_session(&caller(), &parent_key, create_id, &create.encode_to_vec())
            .await
            .unwrap(),
        original_position
    );
    let dispatch = dispatch(&parent);
    let command = wire::DispatchDelegation {
        session_id: dispatch.session_id.clone(),
        operation_id: dispatch.operation_id.clone(),
        child_session_id: dispatch.child_session_id.clone(),
        cascade_policy: dispatch.cascade_policy,
        turn_id: dispatch.turn_id.clone(),
        delegate_binding: dispatch.delegate_binding.clone(),
        child_creation: dispatch.child_creation.clone(),
    };
    let dispatch_id = command_id();
    let recorded = runtime
        .dispatch_delegation(&caller(), &parent_key, dispatch_id, &command.encode_to_vec())
        .await
        .unwrap();
    assert_eq!(recorded.ordinal, 2);
    assert!(recorded.inserted);
    assert!(
        !runtime
            .dispatch_delegation(&caller(), &parent_key, dispatch_id, &command.encode_to_vec())
            .await
            .unwrap()
            .inserted
    );
    drop(runtime);
    assert!(
        harness
            .runtime()
            .recover_child(&caller(), &parent_key, "delegate-operation")
            .await
            .unwrap()
            .inserted
    );
    assert!(
        !harness
            .runtime()
            .recover_child(&caller(), &parent_key, "delegate-operation")
            .await
            .unwrap()
            .inserted
    );
    assert_eq!(harness.child_events().await.len(), 2);
}

#[tokio::test]
async fn an_existing_child_with_different_original_creation_is_a_conflict() {
    let harness = Harness::new().await;
    harness.seed_parent().await;
    let child_key = key("child");
    let unrelated = started("child", b"conflicting plan");
    harness
        .store
        .append_stream(AppendStreamRequest {
            stream_id: &child_key,
            stream_write_precondition: StreamWritePrecondition::NoStream,
            events: vec![harness.fact(&child_key, command_id(), 0, &unrelated).await],
        })
        .await
        .unwrap();
    let original = harness.child_events().await;
    assert!(matches!(
        harness
            .runtime()
            .recover_child(&caller(), &key("parent"), "delegate-operation")
            .await,
        Err(trogon_session_store::SessionError::CreationConflict)
    ));
    assert_eq!(harness.child_events().await, original);
}

#[tokio::test]
async fn current_target_policy_is_rechecked_even_for_an_existing_exact_child() {
    let harness = Harness::new().await;
    harness.seed_parent().await;
    harness
        .runtime()
        .recover_child(&caller(), &key("parent"), "delegate-operation")
        .await
        .unwrap();
    let original = harness.child_events().await;
    harness.admission.deny_child.store(true, Ordering::SeqCst);
    assert!(matches!(
        harness
            .runtime()
            .recover_child(&caller(), &key("parent"), "delegate-operation")
            .await,
        Err(trogon_session_store::SessionError::Admission(_))
    ));
    assert_eq!(harness.child_events().await, original);
}

#[tokio::test]
async fn expired_original_source_authority_cannot_create_a_child() {
    let harness = Harness::new().await;
    harness.seed_parent().await;
    harness.admission.expired_source.store(true, Ordering::SeqCst);
    assert!(matches!(
        harness
            .runtime()
            .recover_child(&caller(), &key("parent"), "delegate-operation")
            .await,
        Err(trogon_session_store::SessionError::AuthorityExpired)
    ));
    assert_eq!(harness.admission.child_checks.load(Ordering::SeqCst), 0);
    assert!(harness.child_events().await.is_empty());
}

#[tokio::test]
async fn original_source_withdrawal_cancels_pending_child_admission() {
    let harness = Harness::new().await;
    harness.seed_parent().await;
    harness.admission.block_child.store(true, Ordering::SeqCst);
    let runtime = harness.runtime();
    let task = tokio::spawn(async move {
        runtime
            .recover_child(&caller(), &key("parent"), "delegate-operation")
            .await
    });
    harness.admission.child_entered.notified().await;
    harness.admission.withdrawn.send(true).unwrap();
    assert!(matches!(
        task.await.unwrap(),
        Err(trogon_session_store::SessionError::AuthorityWithdrawn)
    ));
    assert!(harness.child_events().await.is_empty());
}

#[tokio::test]
async fn malformed_original_wire_is_refused_before_native_creation() {
    let harness = Harness::new().await;
    let original = started("parent", b"original parent plan");
    let create = wire::CreateSession {
        session_id: original.session_id,
        execution_plan: original.execution_plan,
        checkout_ref: original.checkout_ref,
        candidate_ref: original.candidate_ref,
    };
    let mut unknown = create.encode_to_vec();
    unknown.extend_from_slice(&[0xf8, 0x07, 0x01]);
    assert!(matches!(
        harness
            .runtime()
            .create_session(&caller(), &key("parent"), command_id(), &unknown)
            .await,
        Err(trogon_session_store::SessionError::MalformedWire)
    ));
    let mut duplicate = create.encode_to_vec();
    duplicate.extend_from_slice(&[0x0a, 0x06, b'p', b'a', b'r', b'e', b'n', b't']);
    assert!(matches!(
        harness
            .runtime()
            .create_session(&caller(), &key("parent"), command_id(), &duplicate)
            .await,
        Err(trogon_session_store::SessionError::MalformedWire)
    ));
    assert!(
        harness
            .store
            .read_stream(ReadStreamRequest {
                stream_id: &key("parent"),
                from: ReadFrom::Beginning
            })
            .await
            .unwrap()
            .events
            .is_empty()
    );
}

#[tokio::test]
async fn unknown_original_parent_fields_are_never_normalized_into_recovery_authority() {
    let harness = Harness::new().await;
    let parent_key = key("parent");
    let parent = started("parent", b"original parent plan");
    let dispatch = dispatch(&parent);
    let id = command_id().event_id(0);
    let mut original = parent.encode_to_vec();
    original.extend_from_slice(&[0xf8, 0x07, 0x01]);
    let content = harness
        .codec
        .seal(&parent_key, id, wire::SessionStarted::FULL_NAME, &original)
        .await
        .unwrap();
    harness
        .store
        .append_stream(AppendStreamRequest {
            stream_id: &parent_key,
            stream_write_precondition: StreamWritePrecondition::NoStream,
            events: vec![
                Event {
                    id,
                    r#type: wire::SessionStarted::FULL_NAME.to_owned(),
                    content,
                    headers: Headers::empty(),
                },
                harness.fact(&parent_key, command_id(), 0, &dispatch).await,
            ],
        })
        .await
        .unwrap();
    assert!(matches!(
        harness
            .runtime()
            .recover_child(&caller(), &parent_key, "delegate-operation")
            .await,
        Err(trogon_session_store::SessionError::MalformedWire)
    ));
    assert!(harness.child_events().await.is_empty());
}

#[tokio::test]
async fn concurrent_reconcilers_observe_one_atomic_child_creation() {
    let harness = Harness::new().await;
    harness.seed_parent().await;
    let (first, observed) = harness.fault_runtime(0);
    let second = SessionRuntime::new(
        Arc::clone(&observed),
        Arc::clone(&harness.admission),
        Arc::clone(&harness.codec),
    );
    let actor = caller();
    let parent = key("parent");
    let (first, second) = tokio::join!(
        first.recover_child(&actor, &parent, "delegate-operation"),
        second.recover_child(&actor, &parent, "delegate-operation")
    );
    let first = first.unwrap();
    let second = second.unwrap();
    assert_ne!(first.inserted, second.inserted);
    assert_eq!(first.creation_position, second.creation_position);
    assert_eq!(harness.child_events().await.len(), 2);
}

type NativeStore = JetStreamStore<trogon_session_store::SessionSubjectResolver>;
type NativeAppendError = trogon_decider_nats::JetStreamStoreError<trogon_session_store::SessionNativeError>;

#[derive(Debug, thiserror::Error)]
enum FaultError {
    #[error("fixture acknowledgement lost")]
    LostAck,
    #[error(transparent)]
    Native(#[from] NativeAppendError),
}

struct FaultStore {
    store: Arc<NativeStore>,
    codec: Arc<FixtureCodec>,
    mode: AtomicUsize,
    child_appends: AtomicUsize,
}

impl StreamRead<SessionKey> for FaultStore {
    type Error = <NativeStore as StreamRead<SessionKey>>::Error;

    async fn read_stream(
        &self,
        request: ReadStreamRequest<'_, SessionKey>,
    ) -> Result<trogon_decider_runtime::ReadStreamResponse, Self::Error> {
        self.store.read_stream(request).await
    }
    async fn read_stream_bounded(
        &self,
        request: ReadStreamRequest<'_, SessionKey>,
        maximum: u64,
    ) -> Result<trogon_decider_runtime::ReadStreamResponse, Self::Error> {
        self.store.read_stream_bounded(request, maximum).await
    }
}

impl FaultStore {
    async fn close_parent(&self) {
        let parent_key = key("parent");
        let current = self
            .store
            .read_stream(ReadStreamRequest {
                stream_id: &parent_key,
                from: ReadFrom::Beginning,
            })
            .await
            .unwrap();
        let id = command_id().event_id(0);
        let original = wire::SessionClosed {
            session_id: "parent".to_owned(),
            ..Default::default()
        }
        .encode_to_vec();
        let content = self
            .codec
            .seal(&parent_key, id, wire::SessionClosed::FULL_NAME, &original)
            .await
            .unwrap();
        self.store
            .append_stream(AppendStreamRequest {
                stream_id: &parent_key,
                stream_write_precondition: StreamWritePrecondition::At(current.current_position.unwrap()),
                events: vec![Event {
                    id,
                    r#type: wire::SessionClosed::FULL_NAME.to_owned(),
                    content,
                    headers: Headers::empty(),
                }],
            })
            .await
            .unwrap();
    }
}

impl StreamAppend<SessionKey> for FaultStore {
    type Error = FaultError;

    async fn append_stream(
        &self,
        request: AppendStreamRequest<'_, SessionKey>,
    ) -> Result<trogon_decider_runtime::AppendStreamResponse, Self::Error> {
        let is_child = request.stream_id.session_id() == "child";
        let mode = if is_child {
            self.child_appends.fetch_add(1, Ordering::SeqCst);
            self.mode.swap(0, Ordering::SeqCst)
        } else {
            0
        };
        if mode == 2 {
            return Err(FaultError::LostAck);
        }
        if mode == 3 {
            self.close_parent().await;
        }
        if mode == 4 {
            let mut independently_sealed = Vec::new();
            for event in request.events {
                let original = self
                    .codec
                    .open(request.stream_id, event.id, &event.r#type, &event.content)
                    .await
                    .unwrap();
                let content = self
                    .codec
                    .seal(request.stream_id, event.id, &event.r#type, &original)
                    .await
                    .unwrap();
                assert_ne!(content, event.content);
                independently_sealed.push(Event { content, ..event });
            }
            self.store
                .append_stream(AppendStreamRequest {
                    stream_id: request.stream_id,
                    stream_write_precondition: request.stream_write_precondition,
                    events: independently_sealed,
                })
                .await?;
            return Err(FaultError::LostAck);
        }
        let result = self.store.append_stream(request).await?;
        if mode == 1 {
            return Err(FaultError::LostAck);
        }
        Ok(result)
    }

    fn classify_append_failure(&self, error: &Self::Error) -> trogon_decider_runtime::AppendFailure {
        match error {
            FaultError::Native(error) => self.store.classify_append_failure(error),
            FaultError::LostAck => trogon_decider_runtime::AppendFailure::Fatal,
        }
    }
}

impl Harness {
    fn fault_runtime(
        &self,
        mode: usize,
    ) -> (
        SessionRuntime<FaultStore, FixtureAdmission, FixtureCodec>,
        Arc<FaultStore>,
    ) {
        let store = Arc::new(FaultStore {
            store: Arc::clone(&self.store),
            codec: Arc::clone(&self.codec),
            mode: AtomicUsize::new(mode),
            child_appends: AtomicUsize::new(0),
        });
        (
            SessionRuntime::new(Arc::clone(&store), Arc::clone(&self.admission), Arc::clone(&self.codec)),
            store,
        )
    }
}

#[tokio::test]
async fn unknown_child_append_ack_is_reconciled_from_exact_native_occurrence() {
    let harness = Harness::new().await;
    harness.seed_parent().await;
    let (runtime, store) = harness.fault_runtime(1);
    let recovered = runtime
        .recover_child(&caller(), &key("parent"), "delegate-operation")
        .await
        .unwrap();
    assert!(!recovered.inserted);
    assert_eq!(store.child_appends.load(Ordering::SeqCst), 1);
    assert_eq!(harness.child_events().await.len(), 2);
}

#[tokio::test]
async fn unknown_uncommitted_append_is_reported_without_automatic_retry() {
    let harness = Harness::new().await;
    harness.seed_parent().await;
    let (runtime, store) = harness.fault_runtime(2);
    assert!(matches!(
        runtime
            .recover_child(&caller(), &key("parent"), "delegate-operation")
            .await,
        Err(trogon_session_store::SessionError::AppendUnknown)
    ));
    assert_eq!(store.child_appends.load(Ordering::SeqCst), 1);
    assert!(harness.child_events().await.is_empty());
}

#[tokio::test]
async fn parent_terminal_race_reports_durable_unlaunched_child_then_refuses_retry() {
    let harness = Harness::new().await;
    harness.seed_parent().await;
    let (runtime, store) = harness.fault_runtime(3);
    assert!(matches!(
        runtime
            .recover_child(&caller(), &key("parent"), "delegate-operation")
            .await,
        Err(trogon_session_store::SessionError::SourceChanged)
    ));
    let child = harness.child_events().await;
    assert_eq!(child.len(), 2);
    assert!(
        child
            .iter()
            .all(|fact| fact.event.r#type != wire::ExecutionAttemptReady::FULL_NAME)
    );
    assert!(matches!(
        runtime
            .recover_child(&caller(), &key("parent"), "delegate-operation")
            .await,
        Err(trogon_session_store::SessionError::SourceChanged)
    ));
    assert_eq!(store.child_appends.load(Ordering::SeqCst), 1);
    assert_eq!(harness.child_events().await, child);
}

fn claim_check(value: &[u8]) -> wire::ClaimCheck {
    wire::ClaimCheck {
        storage_ref: wire::StorageLocator {
            value: "fixture-artifact".to_owned(),
        }
        .into(),
        digest: wire::Digest {
            algorithm: "sha256".to_owned(),
            value: Sha256::digest(value).to_vec(),
        }
        .into(),
    }
}

#[tokio::test]
async fn ordinary_started_ready_parent_history_is_preserved_for_current_owner_admission() {
    let harness = Harness::new().await;
    let parent_key = key("parent");
    let parent = started("parent", b"original parent plan");
    let attempt = wire::ExecutionAttemptStarted {
        session_id: "parent".to_owned(),
        execution_attempt_id: "attempt-one".to_owned(),
        session_execution_plan_digest: parent.execution_plan.plan_digest.clone(),
        attempt_number: 1,
        started_at: buffa_types::google::protobuf::Timestamp {
            seconds: 1,
            nanos: 0,
            ..Default::default()
        }
        .into(),
        host_artifact_ref: claim_check(b"fixture host").into(),
        ..Default::default()
    };
    let ready = wire::ExecutionAttemptReady {
        session_id: "parent".to_owned(),
        execution_attempt_id: "attempt-one".to_owned(),
        ready_at: buffa_types::google::protobuf::Timestamp {
            seconds: 2,
            nanos: 0,
            ..Default::default()
        }
        .into(),
        ready_attestation_ref: claim_check(b"fixture ready").into(),
    };
    let dispatch = dispatch(&parent);
    harness
        .store
        .append_stream(AppendStreamRequest {
            stream_id: &parent_key,
            stream_write_precondition: StreamWritePrecondition::NoStream,
            events: vec![
                harness.fact(&parent_key, command_id(), 0, &parent).await,
                harness.fact(&parent_key, command_id(), 0, &attempt).await,
                harness.fact(&parent_key, command_id(), 0, &ready).await,
                harness.fact(&parent_key, command_id(), 0, &dispatch).await,
            ],
        })
        .await
        .unwrap();
    assert!(
        harness
            .runtime()
            .recover_child(&caller(), &parent_key, "delegate-operation")
            .await
            .unwrap()
            .inserted
    );
    let child = harness.child_events().await;
    let original_link = harness
        .codec
        .open(
            &key("child"),
            child[1].event.id,
            &child[1].event.r#type,
            &child[1].event.content,
        )
        .await
        .unwrap();
    assert_eq!(
        wire::ParentLinked::decode_from_slice(&original_link)
            .unwrap()
            .parent_dispatched_at
            .value,
        4
    );
}

#[tokio::test]
async fn conflicting_candidate_or_checkout_is_refused_with_identical_event_identity_and_lineage() {
    for change_candidate in [false, true] {
        let harness = Harness::new().await;
        let dispatch = harness.seed_parent().await;
        let parent = harness
            .store
            .read_stream(ReadStreamRequest {
                stream_id: &key("parent"),
                from: ReadFrom::Beginning,
            })
            .await
            .unwrap();
        let creation_id = CommandId::new(parent.events[1].event.id.as_uuid());
        let child_key = key("child");
        let mut child_started = started("child", b"original child plan");
        if change_candidate {
            child_started.candidate_ref = wire::CandidateRef {
                candidate_id: wire::CandidateId {
                    value: "unexpected-candidate".to_owned(),
                }
                .into(),
                content_digest: wire::Digest {
                    algorithm: "sha256".to_owned(),
                    value: Sha256::digest(b"candidate").to_vec(),
                }
                .into(),
            }
            .into();
        } else {
            child_started.checkout_ref = wire::CheckoutRef {
                uri: "git://example.test/other".to_owned(),
                ..checkout()
            }
            .into();
        }
        let link = wire::ParentLinked {
            session_id: "child".to_owned(),
            parent_session_id: "parent".to_owned(),
            parent_dispatched_at: wire::SessionOrdinal { value: 2 }.into(),
            cascade_policy: dispatch.cascade_policy,
            operation_id: dispatch.operation_id,
            delegate_binding: dispatch.delegate_binding,
        };
        harness
            .store
            .append_stream(AppendStreamRequest {
                stream_id: &child_key,
                stream_write_precondition: StreamWritePrecondition::NoStream,
                events: vec![
                    harness.fact(&child_key, creation_id, 0, &child_started).await,
                    harness.fact(&child_key, creation_id, 1, &link).await,
                ],
            })
            .await
            .unwrap();
        let existing = harness.child_events().await;
        assert!(matches!(
            harness
                .runtime()
                .recover_child(&caller(), &key("parent"), "delegate-operation")
                .await,
            Err(trogon_session_store::SessionError::CreationConflict)
        ));
        assert_eq!(harness.child_events().await, existing);
    }
}

#[tokio::test]
async fn randomized_envelopes_do_not_change_original_child_duplicate_identity() {
    let harness = Harness::new().await;
    harness.seed_parent().await;
    let id = command_id().event_id(0);
    let original = started("child", b"original child plan").encode_to_vec();
    let first = harness
        .codec
        .seal(&key("child"), id, wire::SessionStarted::FULL_NAME, &original)
        .await
        .unwrap();
    let second = harness
        .codec
        .seal(&key("child"), id, wire::SessionStarted::FULL_NAME, &original)
        .await
        .unwrap();
    assert_ne!(first, second);
    let (runtime, store) = harness.fault_runtime(1);
    runtime
        .recover_child(&caller(), &key("parent"), "delegate-operation")
        .await
        .unwrap();
    let existing = harness.child_events().await;
    assert!(
        !harness
            .runtime()
            .recover_child(&caller(), &key("parent"), "delegate-operation")
            .await
            .unwrap()
            .inserted
    );
    assert_eq!(harness.child_events().await, existing);
    assert_eq!(store.child_appends.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn exact_nested_plan_encoding_survives_recovery_and_semantic_reencoding_is_not_a_duplicate() {
    for preseed_reencoded_child in [false, true] {
        let harness = Harness::new().await;
        let parent_key = key("parent");
        let child_key = key("child");
        let parent = started("parent", b"original parent plan");
        let dispatch = dispatch(&parent);
        let dispatch_id = command_id();
        let plan = dispatch.child_creation.execution_plan.encode_to_vec();
        let split = 2 + b"original child plan".len();
        assert_eq!(plan[0], 0x0a);
        assert_eq!(plan[1] as usize, b"original child plan".len());
        let reordered = [&plan[split..], &plan[..split]].concat();
        assert_eq!(
            wire::StoredSessionExecutionPlan::decode_from_slice(&plan).unwrap(),
            wire::StoredSessionExecutionPlan::decode_from_slice(&reordered).unwrap()
        );
        let mut original_dispatch = dispatch.encode_to_vec();
        let positions = original_dispatch
            .windows(plan.len())
            .enumerate()
            .filter_map(|(index, value)| (value == plan).then_some(index))
            .collect::<Vec<_>>();
        assert_eq!(positions.len(), 1);
        original_dispatch[positions[0]..positions[0] + plan.len()].copy_from_slice(&reordered);
        let id = dispatch_id.event_id(0);
        let content = harness
            .codec
            .seal(
                &parent_key,
                id,
                wire::DelegationDispatched::FULL_NAME,
                &original_dispatch,
            )
            .await
            .unwrap();
        harness
            .store
            .append_stream(AppendStreamRequest {
                stream_id: &parent_key,
                stream_write_precondition: StreamWritePrecondition::NoStream,
                events: vec![
                    harness.fact(&parent_key, command_id(), 0, &parent).await,
                    Event {
                        id,
                        r#type: wire::DelegationDispatched::FULL_NAME.to_owned(),
                        content,
                        headers: Headers::empty(),
                    },
                ],
            })
            .await
            .unwrap();
        if preseed_reencoded_child {
            let creation_id = CommandId::new(id.as_uuid());
            let canonical = started("child", b"original child plan");
            let link = wire::ParentLinked {
                session_id: "child".to_owned(),
                parent_session_id: "parent".to_owned(),
                parent_dispatched_at: wire::SessionOrdinal { value: 2 }.into(),
                cascade_policy: dispatch.cascade_policy,
                operation_id: dispatch.operation_id,
                delegate_binding: dispatch.delegate_binding,
            };
            harness
                .store
                .append_stream(AppendStreamRequest {
                    stream_id: &child_key,
                    stream_write_precondition: StreamWritePrecondition::NoStream,
                    events: vec![
                        harness.fact(&child_key, creation_id, 0, &canonical).await,
                        harness.fact(&child_key, creation_id, 1, &link).await,
                    ],
                })
                .await
                .unwrap();
            assert!(matches!(
                harness
                    .runtime()
                    .recover_child(&caller(), &parent_key, "delegate-operation")
                    .await,
                Err(trogon_session_store::SessionError::CreationConflict)
            ));
        } else {
            harness
                .runtime()
                .recover_child(&caller(), &parent_key, "delegate-operation")
                .await
                .unwrap();
            let child = harness.child_events().await;
            let original = harness
                .codec
                .open(
                    &child_key,
                    child[0].event.id,
                    &child[0].event.r#type,
                    &child[0].event.content,
                )
                .await
                .unwrap();
            assert!(original.windows(reordered.len()).any(|value| value == reordered));
            assert!(!original.windows(plan.len()).any(|value| value == plan));
            assert_eq!(
                wire::SessionStarted::decode_from_slice(&original).unwrap(),
                started("child", b"original child plan")
            );
        }
    }
}

#[tokio::test]
async fn declared_binding_preserves_case_dots_and_unicode_without_subject_token_restrictions() {
    let harness = Harness::new().await;
    let parent_key = key("parent");
    let parent = started("parent", b"original parent plan");
    let mut dispatch = dispatch(&parent);
    dispatch.delegate_binding = wire::DelegationBindingRef {
        binding_name: Some("Reviewer.日本語".to_owned()),
        parent_execution_plan_digest: parent.execution_plan.plan_digest.clone(),
    }
    .into();
    harness
        .store
        .append_stream(AppendStreamRequest {
            stream_id: &parent_key,
            stream_write_precondition: StreamWritePrecondition::NoStream,
            events: vec![
                harness.fact(&parent_key, command_id(), 0, &parent).await,
                harness.fact(&parent_key, command_id(), 0, &dispatch).await,
            ],
        })
        .await
        .unwrap();
    harness
        .runtime()
        .recover_child(&caller(), &parent_key, "delegate-operation")
        .await
        .unwrap();
    let child = harness.child_events().await;
    let original_link = harness
        .codec
        .open(
            &key("child"),
            child[1].event.id,
            &child[1].event.r#type,
            &child[1].event.content,
        )
        .await
        .unwrap();
    assert_eq!(
        wire::ParentLinked::decode_from_slice(&original_link)
            .unwrap()
            .delegate_binding
            .binding_name
            .as_deref(),
        Some("Reviewer.日本語")
    );
}

#[tokio::test]
async fn unknown_append_with_independently_sealed_exact_winner_is_reconciled_without_replay() {
    let harness = Harness::new().await;
    harness.seed_parent().await;
    let (runtime, store) = harness.fault_runtime(4);
    let recovered = runtime
        .recover_child(&caller(), &key("parent"), "delegate-operation")
        .await
        .expect("authenticated exact winning creation must satisfy desired state regardless of sealing nonce");
    assert!(!recovered.inserted);
    assert_eq!(store.child_appends.load(Ordering::SeqCst), 1);
    let existing = harness.child_events().await;
    assert_eq!(existing.len(), 2);
    assert!(
        !runtime
            .recover_child(&caller(), &key("parent"), "delegate-operation")
            .await
            .unwrap()
            .inserted
    );
    assert_eq!(store.child_appends.load(Ordering::SeqCst), 1);
    assert_eq!(harness.child_events().await, existing);
}

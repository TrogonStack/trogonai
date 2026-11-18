use super::*;
use trogon_session_store::{AttemptSource, SessionError};

fn encoded<T: Message + MessageName>(value: T) -> (&'static str, Vec<u8>) {
    (T::FULL_NAME, value.encode_to_vec())
}

impl Harness {
    async fn append_original_history(&self, parent_key: &SessionKey, originals: Vec<(&str, Vec<u8>)>) {
        let mut events = Vec::new();
        let command = command_id();
        for (index, (kind, original)) in originals.into_iter().enumerate() {
            let id = command.event_id(index);
            let content = self.codec.seal(parent_key, id, kind, &original).await.unwrap();
            events.push(Event {
                id,
                r#type: kind.to_owned(),
                content,
                headers: Headers::empty(),
            });
        }
        self.store
            .append_stream(AppendStreamRequest {
                stream_id: parent_key,
                stream_write_precondition: StreamWritePrecondition::NoStream,
                events,
            })
            .await
            .unwrap();
    }
}

fn attempt(
    parent: &wire::SessionStarted,
    id: &str,
    number: u64,
    previous: Option<&str>,
) -> wire::ExecutionAttemptStarted {
    wire::ExecutionAttemptStarted {
        session_id: parent.session_id.clone(),
        execution_attempt_id: id.to_owned(),
        session_execution_plan_digest: parent.execution_plan.plan_digest.clone(),
        attempt_number: number,
        previous_attempt_id: previous.map(ToOwned::to_owned),
        started_at: buffa_types::google::protobuf::Timestamp {
            seconds: 1,
            nanos: 0,
            ..Default::default()
        }
        .into(),
        host_artifact_ref: claim_check(b"original host evidence").into(),
        ..Default::default()
    }
}

fn ready(session: &str, id: &str) -> wire::ExecutionAttemptReady {
    wire::ExecutionAttemptReady {
        session_id: session.to_owned(),
        execution_attempt_id: id.to_owned(),
        ready_at: buffa_types::google::protobuf::Timestamp {
            seconds: 2,
            nanos: 0,
            ..Default::default()
        }
        .into(),
        ready_attestation_ref: claim_check(b"original ready evidence").into(),
    }
}

fn ended(session: &str, id: &str) -> wire::ExecutionAttemptEnded {
    wire::ExecutionAttemptEnded {
        session_id: session.to_owned(),
        execution_attempt_id: id.to_owned(),
        outcome: wire::AttemptOutcome::ATTEMPT_OUTCOME_FAILED.into(),
        detail: Some("worker exited".to_owned()),
        ended_at: buffa_types::google::protobuf::Timestamp {
            seconds: 3,
            nanos: 0,
            ..Default::default()
        }
        .into(),
    }
}

fn reservation(session: &str, operation: &str) -> wire::OperationReserved {
    wire::OperationReserved {
        session_id: session.to_owned(),
        operation_id: operation.to_owned(),
        request_digest: wire::Digest {
            algorithm: "sha256".to_owned(),
            value: Sha256::digest(b"delegate request").to_vec(),
        }
        .into(),
        operation_kind: wire::OperationKind::OPERATION_KIND_CHILD_SESSION_DELEGATION.into(),
        turn_id: "turn".to_owned(),
    }
}

#[derive(Default)]
struct ObservedAttemptAdmission {
    base: FixtureAdmission,
    expected_start: Vec<u8>,
    expected_ready: Option<Vec<u8>>,
    calls: AtomicUsize,
}

impl SessionAdmission for ObservedAttemptAdmission {
    type Error = FixturePolicyError;
    type Lease = FixtureLease;
    async fn authorize_source(
        &self,
        _caller: &SubjectPrincipal,
        _key: &SessionKey,
    ) -> Result<Self::Lease, Self::Error> {
        Ok(FixtureLease::new(&self.base, false))
    }
    async fn authorize_create(&self, _material: CreateAdmission<'_>) -> Result<Self::Lease, Self::Error> {
        Err(FixturePolicyError)
    }
    async fn authorize_dispatch(&self, _material: DispatchAdmission<'_>) -> Result<Self::Lease, Self::Error> {
        Err(FixturePolicyError)
    }
    async fn authorize_child(&self, material: ChildAdmission<'_>) -> Result<Self::Lease, Self::Error> {
        let AttemptSource {
            started,
            original_started,
            ready,
            original_ready,
        } = material
            .attempt
            .expect("the current attempt must reach owner admission");
        assert_eq!(original_started, self.expected_start);
        assert_eq!(started.encode_to_vec(), self.expected_start);
        assert_eq!(original_ready, self.expected_ready.as_deref());
        assert_eq!(ready.map(Message::encode_to_vec), self.expected_ready);
        self.calls.fetch_add(1, Ordering::SeqCst);
        Err(FixturePolicyError)
    }
}

#[tokio::test]
async fn ended_attempts_restart_with_exact_predecessor_and_only_current_evidence_reaches_admission() {
    for current_ready in [false, true] {
        let harness = Harness::new().await;
        let parent_key = key("parent");
        let parent = started("parent", b"original parent plan");
        let first = attempt(&parent, "first", 1, None);
        let current = attempt(&parent, "second", 2, Some("first"));
        let current_ready_fact = ready("parent", "second");
        let mut originals = vec![
            encoded(parent.clone()),
            encoded(first),
            encoded(ready("parent", "first")),
            encoded(ended("parent", "first")),
            encoded(current.clone()),
        ];
        if current_ready {
            originals.push(encoded(current_ready_fact.clone()));
        }
        originals.push(encoded(reservation("parent", "delegate-operation")));
        originals.push(encoded(dispatch(&parent)));
        harness.append_original_history(&parent_key, originals).await;
        let authority = Arc::new(ObservedAttemptAdmission {
            base: FixtureAdmission::default(),
            expected_start: current.encode_to_vec(),
            expected_ready: current_ready.then(|| current_ready_fact.encode_to_vec()),
            calls: AtomicUsize::new(0),
        });
        let runtime = SessionRuntime::new(
            Arc::clone(&harness.store),
            Arc::clone(&authority),
            Arc::clone(&harness.codec),
        );
        assert!(matches!(
            runtime
                .recover_child(&caller(), &parent_key, "delegate-operation")
                .await,
            Err(SessionError::Admission(_))
        ));
        assert_eq!(authority.calls.load(Ordering::SeqCst), 1);
        assert!(harness.child_events().await.is_empty());
    }
}

#[tokio::test]
async fn broken_attempt_lineage_and_invalid_claims_are_refused_before_target_admission() {
    let mut cases = Vec::new();
    let parent = started("parent", b"original parent plan");
    let first = attempt(&parent, "first", 1, None);
    let mut wrong_number = first.clone();
    wrong_number.attempt_number = 2;
    cases.push(vec![encoded(wrong_number)]);
    let mut wrong_plan = first.clone();
    wrong_plan.session_execution_plan_digest = plan(b"other plan").plan_digest;
    cases.push(vec![encoded(wrong_plan)]);
    let mut invalid_claim = first.clone();
    invalid_claim.host_artifact_ref = wire::ClaimCheck {
        storage_ref: wire::StorageLocator { value: String::new() }.into(),
        digest: claim_check(b"x").digest,
    }
    .into();
    cases.push(vec![encoded(invalid_claim)]);
    let mut invalid_timestamp = first.clone();
    invalid_timestamp.started_at = buffa_types::google::protobuf::Timestamp {
        seconds: 1,
        nanos: -1,
        ..Default::default()
    }
    .into();
    cases.push(vec![encoded(invalid_timestamp)]);
    let mut wrong_ready = ready("parent", "another");
    cases.push(vec![encoded(first.clone()), encoded(wrong_ready.clone())]);
    wrong_ready = ready("parent", "first");
    wrong_ready.ready_attestation_ref = wire::ClaimCheck {
        storage_ref: wire::StorageLocator {
            value: "artifact".to_owned(),
        }
        .into(),
        digest: wire::Digest {
            algorithm: "sha256".to_owned(),
            value: vec![0; 31],
        }
        .into(),
    }
    .into();
    cases.push(vec![encoded(first.clone()), encoded(wrong_ready)]);
    let mut invalid_ready_time = ready("parent", "first");
    invalid_ready_time.ready_at = buffa_types::google::protobuf::Timestamp {
        seconds: 2,
        nanos: 1_000_000_000,
        ..Default::default()
    }
    .into();
    cases.push(vec![encoded(first.clone()), encoded(invalid_ready_time)]);
    cases.push(vec![
        encoded(first.clone()),
        encoded(ready("parent", "first")),
        encoded(ready("parent", "first")),
    ]);
    let mut wrong_end = ended("parent", "another");
    cases.push(vec![encoded(first.clone()), encoded(wrong_end.clone())]);
    wrong_end = ended("parent", "first");
    wrong_end.outcome = wire::AttemptOutcome::ATTEMPT_OUTCOME_UNSPECIFIED.into();
    cases.push(vec![encoded(first.clone()), encoded(wrong_end)]);
    let mut invalid_end_time = ended("parent", "first");
    invalid_end_time.ended_at = buffa_types::google::protobuf::Timestamp {
        seconds: 3,
        nanos: -1,
        ..Default::default()
    }
    .into();
    cases.push(vec![encoded(first.clone()), encoded(invalid_end_time)]);
    cases.push(vec![
        encoded(first.clone()),
        encoded(ended("parent", "first")),
        encoded(attempt(&parent, "second", 2, Some("unrelated"))),
    ]);
    cases.push(vec![
        encoded(first.clone()),
        encoded(ended("parent", "first")),
        encoded(attempt(&parent, "first", 2, Some("first"))),
    ]);
    for mut suffix in cases {
        let harness = Harness::new().await;
        let parent_key = key("parent");
        let mut originals = vec![encoded(parent.clone())];
        originals.append(&mut suffix);
        harness.append_original_history(&parent_key, originals).await;
        assert!(matches!(
            harness
                .runtime()
                .recover_child(&caller(), &parent_key, "delegate-operation")
                .await,
            Err(SessionError::UnsupportedHistory)
        ));
        assert_eq!(harness.admission.child_checks.load(Ordering::SeqCst), 0);
        assert!(harness.child_events().await.is_empty());
    }
}

#[tokio::test]
async fn terminal_reasons_and_child_reservation_source_invariants_are_enforced_on_replay() {
    let parent = started("parent", b"original parent plan");
    let mut cases = Vec::new();
    cases.push((
        vec![encoded(wire::SessionCancelled {
            session_id: "parent".to_owned(),
            reason: wire::SessionCancellationReason::SESSION_CANCELLATION_REASON_POLICY.into(),
            ..Default::default()
        })],
        true,
    ));
    cases.push((
        vec![encoded(wire::SessionCancelled {
            session_id: "parent".to_owned(),
            reason: wire::SessionCancellationReason::SESSION_CANCELLATION_REASON_UNSPECIFIED.into(),
            ..Default::default()
        })],
        false,
    ));
    cases.push((
        vec![encoded(wire::SessionCancelled {
            session_id: "foreign".to_owned(),
            reason: wire::SessionCancellationReason::SESSION_CANCELLATION_REASON_POLICY.into(),
            ..Default::default()
        })],
        false,
    ));
    cases.push((
        vec![encoded(wire::SessionFailed {
            session_id: "parent".to_owned(),
            reason: wire::SessionFailureReason::SESSION_FAILURE_REASON_EXECUTION_ERROR.into(),
            ..Default::default()
        })],
        true,
    ));
    cases.push((
        vec![encoded(wire::SessionFailed {
            session_id: "parent".to_owned(),
            reason: wire::SessionFailureReason::SESSION_FAILURE_REASON_UNSPECIFIED.into(),
            ..Default::default()
        })],
        false,
    ));
    cases.push((
        vec![encoded(wire::SessionFailed {
            session_id: "foreign".to_owned(),
            reason: wire::SessionFailureReason::SESSION_FAILURE_REASON_EXECUTION_ERROR.into(),
            ..Default::default()
        })],
        false,
    ));
    cases.push((
        vec![encoded(wire::SessionClosed {
            session_id: "foreign".to_owned(),
            ..Default::default()
        })],
        false,
    ));
    cases.push((
        vec![
            encoded(wire::SessionClosed {
                session_id: "parent".to_owned(),
                ..Default::default()
            }),
            encoded(reservation("parent", "later")),
        ],
        false,
    ));
    cases.push((
        vec![
            encoded(reservation("parent", "same")),
            encoded(reservation("parent", "same")),
        ],
        false,
    ));
    let mut wrong_kind = reservation("parent", "delegate-operation");
    wrong_kind.operation_kind = wire::OperationKind::OPERATION_KIND_TOOL.into();
    cases.push((vec![encoded(wrong_kind)], false));
    let mut wrong_digest = reservation("parent", "delegate-operation");
    wrong_digest.request_digest = wire::Digest {
        algorithm: "sha256".to_owned(),
        value: Vec::new(),
    }
    .into();
    cases.push((vec![encoded(wrong_digest)], false));
    for (suffix, terminal) in cases {
        let harness = Harness::new().await;
        let mut originals = vec![encoded(parent.clone()), encoded(dispatch(&parent))];
        originals.extend(suffix);
        harness.append_original_history(&key("parent"), originals).await;
        let result = harness
            .runtime()
            .recover_child(&caller(), &key("parent"), "delegate-operation")
            .await;
        if terminal {
            assert!(matches!(result, Err(SessionError::SourceChanged)));
        } else {
            assert!(matches!(result, Err(SessionError::UnsupportedHistory)));
        }
        assert!(harness.child_events().await.is_empty());
        assert_eq!(harness.admission.child_checks.load(Ordering::SeqCst), 0);
    }
}

#[tokio::test]
async fn invalid_creation_projection_digest_binding_and_duplicate_dispatches_never_spawn_children() {
    let parent = started("parent", b"original parent plan");
    let mut cases = Vec::new();
    let mut wrong_checkout = parent.clone();
    wrong_checkout.checkout_ref = wire::CheckoutRef {
        checkout_id: String::new(),
        ..checkout()
    }
    .into();
    cases.push(vec![encoded(wrong_checkout)]);
    let mut wrong_hash = parent.clone();
    wrong_hash.execution_plan = wire::StoredSessionExecutionPlan {
        plan_bytes: b"changed original".to_vec(),
        plan_digest: parent.execution_plan.plan_digest.clone(),
    }
    .into();
    cases.push(vec![encoded(wrong_hash)]);
    let mut wrong_candidate = parent.clone();
    wrong_candidate.candidate_ref = wire::CandidateRef {
        candidate_id: wire::CandidateId {
            value: "candidate".to_owned(),
        }
        .into(),
        content_digest: wire::Digest {
            algorithm: "sha256".to_owned(),
            value: vec![0; 31],
        }
        .into(),
    }
    .into();
    cases.push(vec![encoded(wrong_candidate)]);
    let mut wrong_binding = dispatch(&parent);
    wrong_binding.delegate_binding = wire::DelegationBindingRef {
        binding_name: Some(String::new()),
        parent_execution_plan_digest: parent.execution_plan.plan_digest.clone(),
    }
    .into();
    cases.push(vec![encoded(parent.clone()), encoded(wrong_binding)]);
    let mut wrong_cascade = dispatch(&parent);
    wrong_cascade.cascade_policy = wire::CascadePolicy::CASCADE_POLICY_UNSPECIFIED.into();
    cases.push(vec![encoded(parent.clone()), encoded(wrong_cascade)]);
    let mut self_dispatch = dispatch(&parent);
    self_dispatch.child_session_id = "parent".to_owned();
    cases.push(vec![encoded(parent.clone()), encoded(self_dispatch)]);
    let mut reused_child = dispatch(&parent);
    reused_child.operation_id = "another-operation".to_owned();
    cases.push(vec![
        encoded(parent.clone()),
        encoded(dispatch(&parent)),
        encoded(reused_child),
    ]);
    let mut reused_operation = dispatch(&parent);
    reused_operation.child_session_id = "another-child".to_owned();
    cases.push(vec![
        encoded(parent.clone()),
        encoded(dispatch(&parent)),
        encoded(reused_operation),
    ]);
    let self_link = wire::ParentLinked {
        session_id: "parent".to_owned(),
        parent_session_id: "parent".to_owned(),
        parent_dispatched_at: wire::SessionOrdinal { value: 1 }.into(),
        cascade_policy: wire::CascadePolicy::CASCADE_POLICY_INDEPENDENT.into(),
        operation_id: "operation".to_owned(),
        delegate_binding: dispatch(&parent).delegate_binding,
    };
    cases.push(vec![encoded(parent.clone()), encoded(self_link)]);
    cases.push(vec![
        encoded(parent.clone()),
        encoded(wire::Compacted {
            session_id: "parent".to_owned(),
            ..Default::default()
        }),
    ]);
    for originals in cases {
        let harness = Harness::new().await;
        harness.append_original_history(&key("parent"), originals).await;
        assert!(matches!(
            harness
                .runtime()
                .recover_child(&caller(), &key("parent"), "delegate-operation")
                .await,
            Err(SessionError::UnsupportedHistory)
        ));
        assert_eq!(harness.admission.child_checks.load(Ordering::SeqCst), 0);
        assert!(harness.child_events().await.is_empty());
    }
}

#[tokio::test]
async fn selected_candidate_and_large_child_plan_are_copied_without_reencoding() {
    let harness = Harness::new().await;
    let parent = started("parent", b"original parent plan");
    let child_plan = plan(&[0x42; 512]);
    let candidate = wire::CandidateRef {
        candidate_id: wire::CandidateId {
            value: "candidate-selected".to_owned(),
        }
        .into(),
        content_digest: wire::Digest {
            algorithm: "sha256".to_owned(),
            value: Sha256::digest(b"candidate source").to_vec(),
        }
        .into(),
    };
    let mut dispatched = dispatch(&parent);
    dispatched.child_creation = wire::ChildSessionCreation {
        execution_plan: child_plan.clone().into(),
        checkout_ref: checkout().into(),
        candidate_ref: candidate.clone().into(),
    }
    .into();
    harness
        .append_original_history(&key("parent"), vec![encoded(parent), encoded(dispatched)])
        .await;
    harness
        .runtime()
        .recover_child(&caller(), &key("parent"), "delegate-operation")
        .await
        .unwrap();
    let child = harness.child_events().await;
    let original = harness
        .codec
        .open(
            &key("child"),
            child[0].event.id,
            &child[0].event.r#type,
            &child[0].event.content,
        )
        .await
        .unwrap();
    let decoded = wire::SessionStarted::decode_from_slice(&original).unwrap();
    assert_eq!(decoded.execution_plan.as_option(), Some(&child_plan));
    assert_eq!(decoded.candidate_ref.as_option(), Some(&candidate));
    assert!(
        !harness
            .runtime()
            .recover_child(&caller(), &key("parent"), "delegate-operation")
            .await
            .unwrap()
            .inserted
    );
    assert_eq!(harness.child_events().await, child);
}

#[test]
fn session_keys_refuse_subject_escape_and_keep_valid_namespace_identity() {
    let namespace = NatsToken::new("fixture").unwrap();
    for invalid in ["", "a.b", "*", ">", "with space", "日本語", &"x".repeat(129)] {
        assert!(matches!(
            SessionKey::new(namespace.clone(), invalid),
            Err(SessionError::InvalidKey)
        ));
    }
    let valid = SessionKey::new(namespace.clone(), "sess_ABC-123").unwrap();
    assert_eq!(valid.namespace(), &namespace);
    assert_eq!(valid.session_id(), "sess_ABC-123");
    assert_eq!(valid.subject(), "fixture.session.sessions.sess_ABC-123");
    assert_eq!(valid.to_string(), valid.subject());
}

#[derive(Clone, Copy)]
enum Interruption {
    LostAfterCommit,
    LostBeforeCommit,
    TerminalWon,
    ConflictingIntentWon,
}

struct InterruptedWriter {
    native: Arc<NativeStore>,
    codec: Arc<FixtureCodec>,
    kind: &'static str,
    interruption: Interruption,
    intercepted: AtomicUsize,
}

impl StreamRead<SessionKey> for InterruptedWriter {
    type Error = <NativeStore as StreamRead<SessionKey>>::Error;
    async fn read_stream(
        &self,
        request: ReadStreamRequest<'_, SessionKey>,
    ) -> Result<trogon_decider_runtime::ReadStreamResponse, Self::Error> {
        self.native.read_stream(request).await
    }
    async fn read_stream_bounded(
        &self,
        request: ReadStreamRequest<'_, SessionKey>,
        maximum: u64,
    ) -> Result<trogon_decider_runtime::ReadStreamResponse, Self::Error> {
        self.native.read_stream_bounded(request, maximum).await
    }
}

impl StreamAppend<SessionKey> for InterruptedWriter {
    type Error = FaultError;
    async fn append_stream(
        &self,
        request: AppendStreamRequest<'_, SessionKey>,
    ) -> Result<trogon_decider_runtime::AppendStreamResponse, Self::Error> {
        if request.events.first().is_none_or(|event| event.r#type != self.kind)
            || self.intercepted.fetch_add(1, Ordering::SeqCst) > 0
        {
            return self.native.append_stream(request).await.map_err(FaultError::Native);
        }
        match self.interruption {
            Interruption::LostBeforeCommit => Err(FaultError::LostAck),
            Interruption::LostAfterCommit => {
                self.native.append_stream(request).await?;
                Err(FaultError::LostAck)
            }
            Interruption::TerminalWon | Interruption::ConflictingIntentWon => {
                let id = command_id().event_id(0);
                let event = if matches!(self.interruption, Interruption::TerminalWon) {
                    let original = wire::SessionClosed {
                        session_id: request.stream_id.session_id().to_owned(),
                        ..Default::default()
                    }
                    .encode_to_vec();
                    Event {
                        id,
                        r#type: wire::SessionClosed::FULL_NAME.to_owned(),
                        content: self
                            .codec
                            .seal(request.stream_id, id, wire::SessionClosed::FULL_NAME, &original)
                            .await
                            .unwrap(),
                        headers: Headers::empty(),
                    }
                } else {
                    let submitted = &request.events[0];
                    let original = self
                        .codec
                        .open(request.stream_id, submitted.id, &submitted.r#type, &submitted.content)
                        .await
                        .unwrap();
                    let replacement = if self.kind == wire::SessionStarted::FULL_NAME {
                        let mut started = wire::SessionStarted::decode_from_slice(&original).unwrap();
                        started.checkout_ref = wire::CheckoutRef {
                            uri: "git://example.test/competing".to_owned(),
                            ..checkout()
                        }
                        .into();
                        started.encode_to_vec()
                    } else {
                        let mut dispatch = wire::DelegationDispatched::decode_from_slice(&original).unwrap();
                        dispatch.child_session_id = "competing-child".to_owned();
                        dispatch.encode_to_vec()
                    };
                    Event {
                        id,
                        r#type: self.kind.to_owned(),
                        content: self
                            .codec
                            .seal(request.stream_id, id, self.kind, &replacement)
                            .await
                            .unwrap(),
                        headers: Headers::empty(),
                    }
                };
                self.native
                    .append_stream(AppendStreamRequest {
                        stream_id: request.stream_id,
                        stream_write_precondition: request.stream_write_precondition,
                        events: vec![event],
                    })
                    .await?;
                self.native.append_stream(request).await.map_err(FaultError::Native)
            }
        }
    }
    fn classify_append_failure(&self, error: &Self::Error) -> trogon_decider_runtime::AppendFailure {
        match error {
            FaultError::Native(error) => self.native.classify_append_failure(error),
            FaultError::LostAck => trogon_decider_runtime::AppendFailure::Fatal,
        }
    }
}

impl Harness {
    fn interrupted_runtime(
        &self,
        kind: &'static str,
        interruption: Interruption,
    ) -> (
        SessionRuntime<InterruptedWriter, FixtureAdmission, FixtureCodec>,
        Arc<InterruptedWriter>,
    ) {
        let store = Arc::new(InterruptedWriter {
            native: Arc::clone(&self.store),
            codec: Arc::clone(&self.codec),
            kind,
            interruption,
            intercepted: AtomicUsize::new(0),
        });
        (
            SessionRuntime::new(Arc::clone(&store), Arc::clone(&self.admission), Arc::clone(&self.codec)),
            store,
        )
    }
}

fn create_command(parent: &wire::SessionStarted) -> wire::CreateSession {
    wire::CreateSession {
        session_id: parent.session_id.clone(),
        execution_plan: parent.execution_plan.clone(),
        checkout_ref: parent.checkout_ref.clone(),
        candidate_ref: parent.candidate_ref.clone(),
    }
}

fn dispatch_command(parent: &wire::SessionStarted) -> wire::DispatchDelegation {
    let fact = dispatch(parent);
    wire::DispatchDelegation {
        session_id: fact.session_id,
        operation_id: fact.operation_id,
        child_session_id: fact.child_session_id,
        cascade_policy: fact.cascade_policy,
        turn_id: fact.turn_id,
        delegate_binding: fact.delegate_binding,
        child_creation: fact.child_creation,
    }
}

#[tokio::test]
async fn root_creation_reconciles_lost_ack_and_refuses_a_concurrent_different_original() {
    for interruption in [
        Interruption::LostAfterCommit,
        Interruption::LostBeforeCommit,
        Interruption::ConflictingIntentWon,
    ] {
        let harness = Harness::new().await;
        let parent = started("parent", b"original parent plan");
        let original = create_command(&parent).encode_to_vec();
        let id = command_id();
        let (runtime, observed) = harness.interrupted_runtime(wire::SessionStarted::FULL_NAME, interruption);
        let result = runtime.create_session(&caller(), &key("parent"), id, &original).await;
        match interruption {
            Interruption::LostAfterCommit => {
                let position = result.unwrap();
                assert_eq!(
                    runtime
                        .create_session(&caller(), &key("parent"), id, &original)
                        .await
                        .unwrap(),
                    position
                );
            }
            Interruption::LostBeforeCommit => assert!(matches!(result, Err(SessionError::AppendUnknown))),
            Interruption::ConflictingIntentWon => assert!(matches!(result, Err(SessionError::CreationConflict))),
            Interruption::TerminalWon => unreachable!(),
        }
        assert_eq!(observed.intercepted.load(Ordering::SeqCst), 1);
    }
}

#[tokio::test]
async fn parent_dispatch_reconciles_exact_original_or_refuses_the_native_competing_transition() {
    for interruption in [
        Interruption::LostAfterCommit,
        Interruption::LostBeforeCommit,
        Interruption::ConflictingIntentWon,
        Interruption::TerminalWon,
    ] {
        let harness = Harness::new().await;
        let parent = started("parent", b"original parent plan");
        harness
            .runtime()
            .create_session(
                &caller(),
                &key("parent"),
                command_id(),
                &create_command(&parent).encode_to_vec(),
            )
            .await
            .unwrap();
        let command = dispatch_command(&parent);
        let id = command_id();
        let (runtime, observed) = harness.interrupted_runtime(wire::DelegationDispatched::FULL_NAME, interruption);
        let result = runtime
            .dispatch_delegation(&caller(), &key("parent"), id, &command.encode_to_vec())
            .await;
        match interruption {
            Interruption::LostAfterCommit => {
                let recorded = result.unwrap();
                assert!(!recorded.inserted);
                assert_eq!(recorded.ordinal, 2);
                let again = runtime
                    .dispatch_delegation(&caller(), &key("parent"), id, &command.encode_to_vec())
                    .await
                    .unwrap();
                assert!(!again.inserted);
                assert_eq!(again.position, recorded.position);
            }
            Interruption::LostBeforeCommit => assert!(matches!(result, Err(SessionError::AppendUnknown))),
            Interruption::ConflictingIntentWon => assert!(matches!(result, Err(SessionError::CreationConflict))),
            Interruption::TerminalWon => assert!(matches!(result, Err(SessionError::SourceChanged))),
        }
        assert_eq!(observed.intercepted.load(Ordering::SeqCst), 1);
        assert!(harness.child_events().await.is_empty());
    }
}

#[tokio::test]
async fn a_dispatch_identity_cannot_be_reused_for_another_original_or_another_operation_for_the_same_child() {
    let harness = Harness::new().await;
    let parent = started("parent", b"original parent plan");
    harness.seed_parent().await;
    let command = dispatch_command(&parent);
    assert!(matches!(
        harness
            .runtime()
            .dispatch_delegation(&caller(), &key("parent"), command_id(), &command.encode_to_vec())
            .await,
        Err(SessionError::CreationConflict)
    ));
    let mut same_child = command.clone();
    same_child.operation_id = "different-operation".to_owned();
    assert!(matches!(
        harness
            .runtime()
            .dispatch_delegation(&caller(), &key("parent"), command_id(), &same_child.encode_to_vec())
            .await,
        Err(SessionError::CreationConflict)
    ));
    let mut changed_original = command;
    changed_original.child_session_id = "different-child".to_owned();
    assert!(matches!(
        harness
            .runtime()
            .dispatch_delegation(
                &caller(),
                &key("parent"),
                command_id(),
                &changed_original.encode_to_vec()
            )
            .await,
        Err(SessionError::CreationConflict)
    ));
    assert!(matches!(
        harness
            .runtime()
            .recover_child(&caller(), &key("parent"), "bad.operation")
            .await,
        Err(SessionError::MalformedWire)
    ));
    assert!(harness.child_events().await.is_empty());
}

#[derive(Clone, Copy)]
enum BrokenRead {
    ExcessFacts,
    EmptyAdvertisedHistory,
    ForeignStream,
    RegressedPosition,
    DuplicateIdentity,
    IncompleteFrontier,
    OversizedEnvelope,
}

struct BrokenReadBoundary {
    native: Arc<NativeStore>,
    defect: BrokenRead,
}

impl StreamRead<SessionKey> for BrokenReadBoundary {
    type Error = <NativeStore as StreamRead<SessionKey>>::Error;
    async fn read_stream(
        &self,
        request: ReadStreamRequest<'_, SessionKey>,
    ) -> Result<trogon_decider_runtime::ReadStreamResponse, Self::Error> {
        self.read_stream_bounded(request, 1025).await
    }
    async fn read_stream_bounded(
        &self,
        request: ReadStreamRequest<'_, SessionKey>,
        maximum: u64,
    ) -> Result<trogon_decider_runtime::ReadStreamResponse, Self::Error> {
        let mut observed = self.native.read_stream_bounded(request, maximum).await?;
        match self.defect {
            BrokenRead::ExcessFacts => {
                let fact = observed.events[0].clone();
                observed.events.resize(1025, fact);
            }
            BrokenRead::EmptyAdvertisedHistory => observed.events.clear(),
            BrokenRead::ForeignStream => observed.events[0].stream_id = key("foreign").to_string(),
            BrokenRead::RegressedPosition => observed.events[1].stream_position = observed.events[0].stream_position,
            BrokenRead::DuplicateIdentity => observed.events[1].event.id = observed.events[0].event.id,
            BrokenRead::IncompleteFrontier => observed.current_position = Some(observed.events[0].stream_position),
            BrokenRead::OversizedEnvelope => observed.events[0].event.content.resize(262145, 0),
        }
        Ok(observed)
    }
}

impl StreamAppend<SessionKey> for BrokenReadBoundary {
    type Error = NativeAppendError;
    async fn append_stream(
        &self,
        request: AppendStreamRequest<'_, SessionKey>,
    ) -> Result<trogon_decider_runtime::AppendStreamResponse, Self::Error> {
        self.native.append_stream(request).await
    }
    fn classify_append_failure(&self, error: &Self::Error) -> trogon_decider_runtime::AppendFailure {
        self.native.classify_append_failure(error)
    }
}

#[tokio::test]
async fn corrupt_persistence_observations_cannot_grant_child_creation_authority() {
    let harness = Harness::new().await;
    harness.seed_parent().await;
    for defect in [
        BrokenRead::ExcessFacts,
        BrokenRead::EmptyAdvertisedHistory,
        BrokenRead::ForeignStream,
        BrokenRead::RegressedPosition,
        BrokenRead::DuplicateIdentity,
        BrokenRead::IncompleteFrontier,
        BrokenRead::OversizedEnvelope,
    ] {
        let boundary = Arc::new(BrokenReadBoundary {
            native: Arc::clone(&harness.store),
            defect,
        });
        let runtime = SessionRuntime::new(boundary, Arc::clone(&harness.admission), Arc::clone(&harness.codec));
        let result = runtime
            .recover_child(&caller(), &key("parent"), "delegate-operation")
            .await;
        if matches!(defect, BrokenRead::ExcessFacts | BrokenRead::OversizedEnvelope) {
            assert!(matches!(result, Err(SessionError::Limit)));
        } else {
            assert!(matches!(result, Err(SessionError::UnsupportedHistory)));
        }
    }
    assert_eq!(harness.admission.child_checks.load(Ordering::SeqCst), 0);
    assert!(harness.child_events().await.is_empty());
}

struct OversizedSealing;
impl SessionFactCodec for OversizedSealing {
    type Error = FixtureBindingError;
    async fn seal(
        &self,
        _key: &SessionKey,
        _id: EventId,
        _kind: &str,
        _plaintext: &[u8],
    ) -> Result<Vec<u8>, Self::Error> {
        Ok(vec![0; 262145])
    }
    async fn open(
        &self,
        _key: &SessionKey,
        _id: EventId,
        _kind: &str,
        _envelope: &[u8],
    ) -> Result<Vec<u8>, Self::Error> {
        Err(FixtureBindingError)
    }
}

#[tokio::test]
async fn oversized_sealed_creation_is_refused_before_any_native_append() {
    let harness = Harness::new().await;
    let runtime = SessionRuntime::new(
        Arc::clone(&harness.store),
        Arc::clone(&harness.admission),
        Arc::new(OversizedSealing),
    );
    let original = create_command(&started("parent", b"original parent plan")).encode_to_vec();
    assert!(matches!(
        runtime
            .create_session(&caller(), &key("parent"), command_id(), &original)
            .await,
        Err(SessionError::Limit)
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

#[derive(Default)]
struct PausedDispatchAdmission {
    base: FixtureAdmission,
    entered: Notify,
    proceed: Notify,
}
impl SessionAdmission for PausedDispatchAdmission {
    type Error = FixturePolicyError;
    type Lease = FixtureLease;
    async fn authorize_source(&self, caller: &SubjectPrincipal, key: &SessionKey) -> Result<Self::Lease, Self::Error> {
        self.base.authorize_source(caller, key).await
    }
    async fn authorize_create(&self, material: CreateAdmission<'_>) -> Result<Self::Lease, Self::Error> {
        self.base.authorize_create(material).await
    }
    async fn authorize_dispatch(&self, material: DispatchAdmission<'_>) -> Result<Self::Lease, Self::Error> {
        assert_eq!(material.command.operation_id, "delegate-operation");
        self.entered.notify_one();
        self.proceed.notified().await;
        Ok(FixtureLease::new(&self.base, false))
    }
    async fn authorize_child(&self, material: ChildAdmission<'_>) -> Result<Self::Lease, Self::Error> {
        self.base.authorize_child(material).await
    }
}

#[tokio::test]
async fn duplicate_dispatch_refuses_parent_terminal_change_during_current_authorization() {
    let harness = Harness::new().await;
    let parent = started("parent", b"original parent plan");
    let runtime = harness.runtime();
    runtime
        .create_session(
            &caller(),
            &key("parent"),
            command_id(),
            &create_command(&parent).encode_to_vec(),
        )
        .await
        .unwrap();
    let original = dispatch_command(&parent).encode_to_vec();
    let id = command_id();
    runtime
        .dispatch_delegation(&caller(), &key("parent"), id, &original)
        .await
        .unwrap();
    let authority = Arc::new(PausedDispatchAdmission::default());
    let paused = SessionRuntime::new(
        Arc::clone(&harness.store),
        Arc::clone(&authority),
        Arc::clone(&harness.codec),
    );
    let task = tokio::spawn(async move {
        paused
            .dispatch_delegation(&caller(), &key("parent"), id, &original)
            .await
    });
    authority.entered.notified().await;
    let parent_key = key("parent");
    let observed = harness
        .store
        .read_stream(ReadStreamRequest {
            stream_id: &parent_key,
            from: ReadFrom::Beginning,
        })
        .await
        .unwrap();
    harness
        .store
        .append_stream(AppendStreamRequest {
            stream_id: &parent_key,
            stream_write_precondition: StreamWritePrecondition::At(observed.current_position.unwrap()),
            events: vec![
                harness
                    .fact(
                        &parent_key,
                        command_id(),
                        0,
                        &wire::SessionClosed {
                            session_id: "parent".to_owned(),
                            ..Default::default()
                        },
                    )
                    .await,
            ],
        })
        .await
        .unwrap();
    authority.proceed.notify_one();
    assert!(matches!(task.await.unwrap(), Err(SessionError::SourceChanged)));
    assert!(matches!(
        harness
            .runtime()
            .dispatch_delegation(&caller(), &parent_key, id, &dispatch_command(&parent).encode_to_vec())
            .await,
        Err(SessionError::SourceChanged)
    ));
    assert!(harness.child_events().await.is_empty());
}

#[derive(Debug, thiserror::Error)]
#[error("backend reported a write conflict without an observable occurrence")]
struct UnprovenWriteConflict;

struct UnprovenConflictWriter {
    native: Arc<NativeStore>,
    attempts: AtomicUsize,
}

impl StreamRead<SessionKey> for UnprovenConflictWriter {
    type Error = <NativeStore as StreamRead<SessionKey>>::Error;
    async fn read_stream(
        &self,
        request: ReadStreamRequest<'_, SessionKey>,
    ) -> Result<trogon_decider_runtime::ReadStreamResponse, Self::Error> {
        self.native.read_stream(request).await
    }
    async fn read_stream_bounded(
        &self,
        request: ReadStreamRequest<'_, SessionKey>,
        maximum: u64,
    ) -> Result<trogon_decider_runtime::ReadStreamResponse, Self::Error> {
        self.native.read_stream_bounded(request, maximum).await
    }
}

impl StreamAppend<SessionKey> for UnprovenConflictWriter {
    type Error = UnprovenWriteConflict;
    async fn append_stream(
        &self,
        _request: AppendStreamRequest<'_, SessionKey>,
    ) -> Result<trogon_decider_runtime::AppendStreamResponse, Self::Error> {
        self.attempts.fetch_add(1, Ordering::SeqCst);
        Err(UnprovenWriteConflict)
    }
    fn classify_append_failure(&self, _error: &Self::Error) -> trogon_decider_runtime::AppendFailure {
        trogon_decider_runtime::AppendFailure::WriteConflict
    }
}

#[tokio::test]
async fn reported_write_conflicts_without_observable_creation_are_refused_without_retry() {
    let harness = Harness::new().await;
    let conflicting = Arc::new(UnprovenConflictWriter {
        native: Arc::clone(&harness.store),
        attempts: AtomicUsize::new(0),
    });
    let runtime = SessionRuntime::new(
        Arc::clone(&conflicting),
        Arc::clone(&harness.admission),
        Arc::clone(&harness.codec),
    );
    let original = create_command(&started("parent", b"original parent plan")).encode_to_vec();
    assert!(matches!(
        runtime
            .create_session(&caller(), &key("parent"), command_id(), &original)
            .await,
        Err(SessionError::SourceChanged)
    ));
    assert_eq!(conflicting.attempts.load(Ordering::SeqCst), 1);
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

    harness.seed_parent().await;
    assert!(matches!(
        runtime
            .recover_child(&caller(), &key("parent"), "delegate-operation")
            .await,
        Err(SessionError::SourceChanged)
    ));
    assert_eq!(conflicting.attempts.load(Ordering::SeqCst), 2);
    assert!(harness.child_events().await.is_empty());
}

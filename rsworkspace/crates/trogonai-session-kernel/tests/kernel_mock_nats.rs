use buffa::{EnumValue, MessageField};
use buffa_types::google::protobuf::Timestamp;
use bytes::Bytes;
use trogon_nats::jetstream::MockJetStreamKvStore;
use trogonai_session_contracts::{
    Actor, ActorType, SCHEMA_VERSION_V1, SessionCreatedPayload, SessionEvent, SessionEventPayload, SessionId,
};

use trogonai_session_kernel::{
    InMemoryEventLog, MockSessionLease, MockSessionLeaseFactory, SessionKernel, SessionKernelConfig,
    SessionLeaseManager, SessionMutatingOperation, SnapshotStore,
};

fn created_event(session_id: &str, seq: u64, idempotency_key: &str) -> SessionEvent {
    SessionEvent {
        schema_version: SCHEMA_VERSION_V1,
        event_id: format!("evt_{idempotency_key}"),
        session_id: session_id.to_string(),
        seq,
        operation_id: format!("op_{idempotency_key}"),
        correlation_id: format!("corr_{idempotency_key}"),
        idempotency_key: idempotency_key.to_string(),
        actor: MessageField::some(Actor {
            r#type: EnumValue::Known(ActorType::Kernel),
            id: "session-kernel".to_string(),
            ..Actor::default()
        }),
        payload: MessageField::some(SessionEventPayload {
            kind: Some(
                SessionCreatedPayload {
                    title: "Lease test".to_string(),
                    cwd: "/repo".to_string(),
                    ..SessionCreatedPayload::default()
                }
                .into(),
            ),
            ..SessionEventPayload::default()
        }),
        ..SessionEvent::default()
    }
}

fn test_kernel(
    event_log: InMemoryEventLog,
    snapshot_store: MockJetStreamKvStore,
    lease: MockSessionLease,
) -> SessionKernel<InMemoryEventLog, MockJetStreamKvStore, MockSessionLeaseFactory> {
    let config = SessionKernelConfig::default();
    let snapshots = SnapshotStore::new(snapshot_store, config.clone());
    let leases = SessionLeaseManager::new(MockSessionLeaseFactory::new(lease), "node-1");
    SessionKernel::new(config, event_log, snapshots, leases)
}

#[tokio::test]
async fn acquire_release_session_lease_uses_mock_backend() {
    let session_id = SessionId::new("sess_lease").unwrap();
    let kernel = test_kernel(InMemoryEventLog::new(), MockJetStreamKvStore::new(), MockSessionLease::new());

    let guard = kernel
        .acquire_session_lease(&session_id, SessionMutatingOperation::PromptTurn)
        .await
        .unwrap();
    kernel.release_session_lease(guard).await.unwrap();
}

#[tokio::test]
async fn concurrent_mutating_operation_returns_session_busy() {
    let session_id = SessionId::new("sess_busy").unwrap();
    let lease = MockSessionLease::new();
    lease.hold_by_other();
    let kernel = test_kernel(InMemoryEventLog::new(), MockJetStreamKvStore::new(), lease);

    let result = kernel
        .acquire_session_lease(&session_id, SessionMutatingOperation::SwitchModel)
        .await;

    assert!(matches!(
        result,
        Err(trogonai_session_kernel::SessionKernelError::SessionBusy { .. })
    ));
}

#[tokio::test]
async fn append_event_assigns_monotonic_seq_and_persists_snapshot() {
    let session_id = SessionId::new("sess_append").unwrap();
    let snapshot_store = MockJetStreamKvStore::new();
    snapshot_store.enqueue_get_none();
    snapshot_store.enqueue_create_result(Ok(1));

    let kernel = test_kernel(InMemoryEventLog::new(), snapshot_store.clone(), MockSessionLease::new());

    let mut event = created_event("sess_append", 0, "idem_1");
    let appended = kernel.append_event(event).await.unwrap();
    assert_eq!(appended.seq, 1);

    event = created_event("sess_append", 0, "idem_2");
    let appended = kernel.append_event(event).await.unwrap();
    assert_eq!(appended.seq, 2);

    snapshot_store.enqueue_get_some(Bytes::new());
    let materialized = kernel.materialize_state(&session_id).await.unwrap();
    assert_eq!(materialized.last_applied_seq, 2);
}

#[tokio::test]
async fn append_event_idempotent_deduplicates_retries_after_crash() {
    let session_id = SessionId::new("sess_idem").unwrap();
    let event_log = InMemoryEventLog::new();
    let kernel = test_kernel(event_log.clone(), MockJetStreamKvStore::new(), MockSessionLease::new());

    let event = created_event("sess_idem", 0, "idem_retry");
    let first = kernel
        .append_event_idempotent(event.clone(), "idem_retry")
        .await
        .unwrap();

    // Simulate crash after durable append but before downstream side effects complete.
    let retry = kernel
        .append_event_idempotent(event, "idem_retry")
        .await
        .unwrap();

    assert_eq!(first.event_id, retry.event_id);
    assert_eq!(first.seq, retry.seq);

    let events = event_log.read_session_events(&session_id).await.unwrap();
    assert_eq!(events.len(), 1);
}

#[tokio::test]
async fn recovery_rebuilds_state_from_event_log_and_snapshot() {
    let session_id = SessionId::new("sess_recovery").unwrap();
    let event_log = InMemoryEventLog::new();
    let snapshot_store = MockJetStreamKvStore::new();

    let kernel = test_kernel(event_log, snapshot_store, MockSessionLease::new());
    kernel
        .append_event(created_event("sess_recovery", 0, "idem_1"))
        .await
        .unwrap();
    kernel
        .append_event(created_event("sess_recovery", 0, "idem_2"))
        .await
        .unwrap();

    let recovered = kernel.recover(&session_id).await.unwrap();
    assert_eq!(recovered.replayed_events, 2);
    assert_eq!(recovered.snapshot.last_applied_seq, 2);
}

#[tokio::test]
async fn fork_session_records_lineage_on_child_snapshot() {
    let parent_id = SessionId::new("sess_parent").unwrap();
    let child_id = SessionId::new("sess_child").unwrap();
    let kernel = test_kernel(InMemoryEventLog::new(), MockJetStreamKvStore::new(), MockSessionLease::new());

    // Seed the parent with two events so it has a non-trivial history to branch from.
    kernel
        .append_event(created_event("sess_parent", 0, "idem_p1"))
        .await
        .unwrap();
    kernel
        .append_event(created_event("sess_parent", 0, "idem_p2"))
        .await
        .unwrap();

    let actor = Actor {
        r#type: EnumValue::Known(ActorType::Kernel),
        id: "session-kernel".to_string(),
        ..Actor::default()
    };
    let child = kernel
        .fork_session(&parent_id, &child_id, 2, "op_branch", actor, Timestamp::default())
        .await
        .expect("fork_session failed");

    // The branch event is the child's first event; lineage is materialized on its metadata.
    assert_eq!(child.session_id, "sess_child");
    assert_eq!(child.last_applied_seq, 1);
    let session = child.state.as_option().and_then(|s| s.session.as_option()).unwrap();
    assert_eq!(session.parent_session_id.as_deref(), Some("sess_parent"));
    assert_eq!(session.branched_at_seq, Some(2));
    // The child inherits the parent's state (here, the cwd from the parent's
    // SessionCreated); artifacts would likewise be carried by reference.
    assert_eq!(session.cwd, "/repo");
}

#[tokio::test]
async fn record_conversation_emits_transcript_events_and_materializes() {
    use trogonai_session_contracts::CanonicalMessage;

    let session_id = SessionId::new("sess_convo").unwrap();
    let event_log = InMemoryEventLog::new();
    let kernel = test_kernel(event_log.clone(), MockJetStreamKvStore::new(), MockSessionLease::new());

    let actor = Actor {
        r#type: EnumValue::Known(ActorType::Kernel),
        id: "shadow".to_string(),
        ..Actor::default()
    };
    let messages = vec![
        CanonicalMessage {
            message_id: "m0".to_string(),
            role: "user".to_string(),
            ..CanonicalMessage::default()
        },
        CanonicalMessage {
            message_id: "m1".to_string(),
            role: "assistant".to_string(),
            ..CanonicalMessage::default()
        },
    ];

    let snapshot = kernel
        .record_conversation(&session_id, &messages, actor.clone(), Timestamp::default())
        .await
        .unwrap();
    // The conversation is materialized from the emitted transcript events.
    let state = snapshot.state.as_option().unwrap();
    assert_eq!(state.conversation.len(), 2);
    assert_eq!(state.conversation[0].role, "user");
    assert_eq!(state.conversation[1].role, "assistant");
    assert_eq!(snapshot.last_applied_seq, 2);

    // Transcript events are durably in the log (one user, one assistant).
    let events = event_log.read_session_events(&session_id).await.unwrap();
    assert_eq!(events.len(), 2);

    // Re-running is idempotent: stable per-index keys mean no duplicate events.
    kernel
        .record_conversation(&session_id, &messages, actor, Timestamp::default())
        .await
        .unwrap();
    let events_after = event_log.read_session_events(&session_id).await.unwrap();
    assert_eq!(events_after.len(), 2);
}

#[tokio::test]
async fn record_tool_calls_emits_structured_tool_events() {
    use trogonai_session_contracts::session_event_payload::Kind;
    use trogonai_session_contracts::{CanonicalToolCall, TextToolResult, ToolCallResult};

    let session_id = SessionId::new("sess_tools").unwrap();
    let event_log = InMemoryEventLog::new();
    let kernel = test_kernel(event_log.clone(), MockJetStreamKvStore::new(), MockSessionLease::new());
    let actor = Actor {
        r#type: EnumValue::Known(ActorType::Kernel),
        id: "kernel".to_string(),
        ..Actor::default()
    };

    let tool = CanonicalToolCall {
        id: "tool_1".to_string(),
        tool_execution_id: "exec_1".to_string(),
        name: "bash".to_string(),
        input_json: "{\"cmd\":\"ls\"}".to_string(),
        result: MessageField::some(ToolCallResult {
            kind: Some(
                TextToolResult {
                    content: "files".to_string(),
                    ..TextToolResult::default()
                }
                .into(),
            ),
            ..ToolCallResult::default()
        }),
        ..CanonicalToolCall::default()
    };

    let snap = kernel
        .record_tool_calls(&session_id, std::slice::from_ref(&tool), actor.clone(), Timestamp::default())
        .await
        .unwrap();
    let state = snap.state.as_option().unwrap();
    assert_eq!(state.tool_calls.len(), 1);
    assert_eq!(state.tool_calls[0].name, "bash");

    let events = event_log.read_session_events(&session_id).await.unwrap();
    let has = |pred: fn(&Kind) -> bool| {
        events.iter().any(|e| e.payload.as_option().and_then(|p| p.kind.as_ref()).is_some_and(pred))
    };
    assert!(has(|k| matches!(k, Kind::ToolCallRequested(_))));
    assert!(has(|k| matches!(k, Kind::ToolCallCompleted(_))));

    // Idempotent: re-running appends no new events.
    kernel
        .record_tool_calls(&session_id, std::slice::from_ref(&tool), actor, Timestamp::default())
        .await
        .unwrap();
    assert_eq!(event_log.read_session_events(&session_id).await.unwrap().len(), 2);
}

#[tokio::test]
async fn retention_and_terminal_continuity_apis_emit_and_materialize() {
    use trogonai_session_contracts::TerminalContinuity;
    use trogonai_session_contracts::session_event_payload::Kind;

    let session_id = SessionId::new("sess_ret").unwrap();
    let event_log = InMemoryEventLog::new();
    let kernel = test_kernel(event_log.clone(), MockJetStreamKvStore::new(), MockSessionLease::new());
    let actor = Actor {
        r#type: EnumValue::Known(ActorType::Kernel),
        id: "kernel".to_string(),
        ..Actor::default()
    };

    kernel.append_event(created_event("sess_ret", 0, "idem_r1")).await.unwrap();
    kernel.append_event(created_event("sess_ret", 0, "idem_r2")).await.unwrap();

    // Terminal continuity is captured and materialized onto state.terminal.
    let snap = kernel
        .record_terminal_continuity(
            &session_id,
            TerminalContinuity {
                terminal_cwd: "/work".to_string(),
                dirty_files: vec!["a.rs".to_string()],
                ..TerminalContinuity::default()
            },
            "op_term1",
            actor.clone(),
            Timestamp::default(),
        )
        .await
        .unwrap();
    let terminal = snap.state.as_option().and_then(|s| s.terminal.as_option()).unwrap();
    assert_eq!(terminal.terminal_cwd, "/work");
    assert_eq!(terminal.dirty_files, vec!["a.rs".to_string()]);

    // Snapshot checkpoint records snapshot_created at the materialized seq.
    let checkpoint = kernel
        .checkpoint_snapshot(&session_id, "op_snap1", actor.clone(), Timestamp::default())
        .await
        .unwrap();
    assert!(checkpoint.last_applied_seq >= 3);

    // Archive watermark counts events through a seq.
    let archived = kernel
        .archive_events_through(&session_id, 2, "op_arch1", actor, Timestamp::default())
        .await
        .unwrap();
    assert_eq!(archived, 2);

    // All three event kinds are durably recorded.
    let events = event_log.read_session_events(&session_id).await.unwrap();
    let present = |pred: fn(&Kind) -> bool| {
        events
            .iter()
            .any(|e| e.payload.as_option().and_then(|p| p.kind.as_ref()).is_some_and(pred))
    };
    assert!(present(|k| matches!(k, Kind::TerminalContinuityCaptured(_))));
    assert!(present(|k| matches!(k, Kind::SnapshotCreated(_))));
    assert!(present(|k| matches!(k, Kind::EventsArchived(_))));
}

#[tokio::test]
async fn load_snapshot_reads_from_kv_mock() {
    use buffa::Message as _;
    use trogonai_session_contracts::SessionSnapshot;

    let session_id = SessionId::new("sess_load").unwrap();
    let snapshot_store = MockJetStreamKvStore::new();
    let snapshot = SessionSnapshot {
        schema_version: SCHEMA_VERSION_V1,
        session_id: "sess_load".to_string(),
        last_applied_seq: 3,
        ..SessionSnapshot::default()
    };
    snapshot_store.enqueue_get_some(Bytes::from(snapshot.encode_to_vec()));

    let kernel = test_kernel(InMemoryEventLog::new(), snapshot_store, MockSessionLease::new());
    let loaded = kernel.load_snapshot(&session_id).await.unwrap().unwrap();
    assert_eq!(loaded.last_applied_seq, 3);
}

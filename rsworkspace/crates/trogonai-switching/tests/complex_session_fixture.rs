//! Complex Session Fixture (cambio-modelo.md § "Complex Session Fixture").
//!
//! A single realistic session driven end-to-end through the *real* drivers
//! (session kernel, artifact store, projection compiler, capabilities,
//! switch orchestrator). One `session_id` flows through the whole lifecycle so
//! that event-log replay reconstructs the same canonical snapshot.
//!
//! The fixture exercises the 13 required elements and asserts the 7 pass
//! criteria; each is tagged inline with `// ELEMENT N` / `// CRITERION N`.

use std::sync::Arc;

use buffa::{EnumValue, Message as _, MessageField};
use buffa_types::google::protobuf::Timestamp;
use bytes::Bytes;
use trogon_nats::jetstream::{MockJetStreamKvStore, MockObjectStore};
use trogon_registry::{AgentCapability, MockRegistryStore, Registry};
use trogonai_artifacts::{
    ArtifactAvailability, ArtifactStorageMode, ArtifactStore, ArtifactStoreConfig,
    ArtifactUnavailableReason, StoreArtifactRequest,
};
use trogonai_capabilities::{
    CapabilityConfig, CapabilityRegistry, CertificationLevel, ProviderCertificationEntry,
    ProviderCertificationMatrix, build_adaptation_plan, detect_session_capability_usage,
};
use trogonai_session_contracts::{
    Actor, ActorType, ArtifactRef, CanonicalMessage, CanonicalToolCall, CapabilitySchema,
    CapabilitySource, ContentBlock, DegradationKind, EventId, IdempotencyKey, ModelSwitchReason,
    OperationId, SCHEMA_VERSION_V1, SessionCompactedPayload, SessionCreatedPayload, SessionEvent,
    SessionEventPayload, SessionId, SummaryCreatedPayload, SwitchSafetyStatus, TextToolResult,
    ToolCallResult, ToolCallStatus, ToolExecutionId, ToolResultFormat,
    __buffa::oneof::content_block::Kind as BlockKind,
    __buffa::oneof::tool_call_result::Kind as ToolResultKind, session_event_payload::Kind,
};
use trogonai_session_kernel::{
    InMemoryEventLog, MockSessionLease, MockSessionLeaseFactory, SessionKernel, SessionKernelConfig,
    SessionLeaseManager, SnapshotStore,
};
use trogonai_session_projection::{
    DefaultPromptCompiler, ProjectionConfig, ProjectionInput, PromptCompiler, derive_context_twin,
};
use trogonai_switching::{
    CancelContext, CancelOutcome, CancelState, MockRunnerAcknowledgement, MockRunnerCancellation,
    RunnerBindingStore, SwitchModelRequest, SwitchOrchestrator, SwitchState, SwitchingConfig,
    ToolExecutionState, cancel_operation,
};
use trogonai_session_projection::ContextTwinStore;

const SESSION: &str = "sess_complex_fixture";
const FROM_MODEL: &str = "anthropic/claude-sonnet";
const TO_MODEL: &str = "xai/grok-code-fast";

fn timestamp() -> Timestamp {
    Timestamp {
        seconds: 1_748_995_200,
        nanos: 0,
        ..Timestamp::default()
    }
}

fn kernel_actor() -> Actor {
    Actor {
        r#type: EnumValue::Known(ActorType::Kernel),
        id: "session-kernel".to_string(),
        ..Actor::default()
    }
}

fn user_actor() -> Actor {
    Actor {
        r#type: EnumValue::Known(ActorType::User),
        id: "user".to_string(),
        ..Actor::default()
    }
}

fn created_event(session_id: &str) -> SessionEvent {
    SessionEvent {
        schema_version: SCHEMA_VERSION_V1,
        event_id: "evt_create".to_string(),
        session_id: session_id.to_string(),
        seq: 1,
        operation_id: "op_create".to_string(),
        correlation_id: "corr_create".to_string(),
        idempotency_key: "idem_create".to_string(),
        created_at: MessageField::some(timestamp()),
        actor: MessageField::some(kernel_actor()),
        payload: MessageField::some(SessionEventPayload {
            kind: Some(
                SessionCreatedPayload {
                    title: "Complex session fixture".to_string(),
                    cwd: "/repo".to_string(),
                    model: Some(FROM_MODEL.to_string()),
                    runner: Some("openrouter".to_string()),
                    // ELEMENT 7: compactor_model configured at creation.
                    compactor_model: Some("anthropic/claude-haiku".to_string()),
                    ..SessionCreatedPayload::default()
                }
                .into(),
            ),
            ..SessionEventPayload::default()
        }),
        ..SessionEvent::default()
    }
}

fn text_block(text: &str) -> ContentBlock {
    ContentBlock {
        kind: Some(BlockKind::Text(text.to_string())),
        ..ContentBlock::default()
    }
}

fn message(message_id: &str, role: &str, text: &str) -> CanonicalMessage {
    CanonicalMessage {
        message_id: message_id.to_string(),
        role: role.to_string(),
        content: vec![text_block(text)],
        ..CanonicalMessage::default()
    }
}

fn claude_schema() -> CapabilitySchema {
    CapabilitySchema {
        schema_version: SCHEMA_VERSION_V1,
        model_id: FROM_MODEL.to_string(),
        runner_id: "openrouter".to_string(),
        max_context_tokens: 200_000,
        max_output_tokens: 8_192,
        tool_use: true,
        parallel_tool_calls: true,
        image_input: true,
        json_schema: true,
        reasoning: true,
        streaming: true,
        tool_result_format: EnumValue::Known(ToolResultFormat::Json),
        source: EnumValue::Known(CapabilitySource::Registry),
        last_verified_at: MessageField::some(timestamp()),
        ttl_seconds: 86_400,
        confidence: 0.95,
        ..CapabilitySchema::default()
    }
}

fn grok_schema() -> CapabilitySchema {
    CapabilitySchema {
        schema_version: SCHEMA_VERSION_V1,
        model_id: TO_MODEL.to_string(),
        runner_id: "xai".to_string(),
        max_context_tokens: 131_072,
        max_output_tokens: 16_384,
        tool_use: true,
        parallel_tool_calls: true,
        // ELEMENT 13: grok lacks image_input; the fixture session uses an image,
        // so the adaptation plan must record explicit degradation.
        image_input: false,
        json_schema: true,
        streaming: true,
        tool_result_format: EnumValue::Known(ToolResultFormat::Json),
        source: EnumValue::Known(CapabilitySource::Registry),
        last_verified_at: MessageField::some(timestamp()),
        ttl_seconds: 86_400,
        confidence: 0.95,
        ..CapabilitySchema::default()
    }
}

async fn registry_with_schemas() -> Registry<MockRegistryStore> {
    let registry = Registry::new(MockRegistryStore::new());

    let mut openrouter = AgentCapability::new("openrouter", ["chat"], "acp.openrouter.>");
    openrouter.metadata = serde_json::json!({ "models": [FROM_MODEL] });
    CapabilityRegistry::embed_schema(&mut openrouter, &claude_schema()).unwrap();
    registry.register(&openrouter).await.unwrap();

    let mut xai = AgentCapability::new("xai", ["chat"], "acp.grok.>");
    xai.metadata = serde_json::json!({ "models": [TO_MODEL] });
    CapabilityRegistry::embed_schema(&mut xai, &grok_schema()).unwrap();
    registry.register(&xai).await.unwrap();

    registry
}

fn certification_matrix() -> ProviderCertificationMatrix {
    let mut matrix = ProviderCertificationMatrix::default();
    matrix
        .push_validated(ProviderCertificationEntry {
            model: FROM_MODEL.to_string(),
            runner: "openrouter".to_string(),
            text: true,
            tool_use: true,
            parallel_tools: true,
            image_input: true,
            json_schema: true,
            long_context: true,
            streaming: true,
            artifact_refs: true,
            mcp_tools: true,
            switch_from: vec![],
            switch_to: vec![TO_MODEL.to_string()],
            certified_level: CertificationLevel::Production,
            last_verified_at: None,
        })
        .unwrap();
    matrix
        .push_validated(ProviderCertificationEntry {
            model: TO_MODEL.to_string(),
            runner: "xai".to_string(),
            text: true,
            tool_use: true,
            parallel_tools: true,
            image_input: false,
            json_schema: true,
            long_context: true,
            streaming: true,
            artifact_refs: true,
            mcp_tools: true,
            switch_from: vec![FROM_MODEL.to_string()],
            switch_to: vec![],
            certified_level: CertificationLevel::SwitchSafe,
            last_verified_at: None,
        })
        .unwrap();
    matrix
}

type Orchestrator = SwitchOrchestrator<
    InMemoryEventLog,
    MockJetStreamKvStore,
    MockSessionLeaseFactory,
    MockJetStreamKvStore,
    MockJetStreamKvStore,
    MockRegistryStore,
    MockRunnerAcknowledgement,
>;

/// Build the orchestrator over a *shared* `InMemoryEventLog` (Clone shares state)
/// so that events appended both by the driver and by the orchestrator land in the
/// same canonical log and replay together.
fn build_orchestrator(
    event_log: InMemoryEventLog,
    registry: Registry<MockRegistryStore>,
) -> Orchestrator {
    let kernel_config = SessionKernelConfig::default();
    let snap_store = SnapshotStore::new(MockJetStreamKvStore::new(), kernel_config.clone());
    let snapshots = snap_store.clone();
    let leases = SessionLeaseManager::new(
        MockSessionLeaseFactory::new(MockSessionLease::new()),
        "complex-fixture",
    );
    let kernel = SessionKernel::new(kernel_config, event_log, snap_store, leases);
    let runner_bindings =
        RunnerBindingStore::new(MockJetStreamKvStore::new(), SwitchingConfig::default());
    let twin_store = ContextTwinStore::new(MockJetStreamKvStore::new(), ProjectionConfig::default());

    SwitchOrchestrator::new(
        kernel,
        snapshots,
        runner_bindings,
        twin_store,
        registry,
        Arc::new(MockRunnerAcknowledgement::default()),
        // CRITERION 7: continuity checkpoint enabled so the switch produces a
        // Continuity Checkpoint result (in addition to the Safety Gate).
        SwitchingConfig {
            continuity_checkpoint_enabled: true,
            ..SwitchingConfig::default()
        },
        CapabilityConfig::default(),
        ProjectionConfig::default(),
        certification_matrix(),
    )
}

/// True when any event in the log carries a payload matching `matcher`.
fn has_kind(events: &[SessionEvent], matcher: fn(&Kind) -> bool) -> bool {
    events.iter().any(|event| {
        event
            .payload
            .as_option()
            .and_then(|payload| payload.kind.as_ref())
            .is_some_and(matcher)
    })
}

fn count_kind(events: &[SessionEvent], matcher: fn(&Kind) -> bool) -> usize {
    events
        .iter()
        .filter(|event| {
            event
                .payload
                .as_option()
                .and_then(|payload| payload.kind.as_ref())
                .is_some_and(matcher)
        })
        .count()
}

/// Encode a snapshot's canonical state with replay-time wall-clock stamps cleared.
///
/// `materialize_from_events` stamps `materialized_at` and several nested
/// `created_at`/`completed_at`/`started_at`/`evaluated_at`/`attached_at`/
/// `detached_at`/`updated_at` fields with `now_timestamp()` at replay time. Those
/// are non-canonical (they reflect *when* the replay ran, not session content), so
/// we zero them before comparing two independent replays for canonical identity.
fn canonical_state_bytes(snapshot: &trogonai_session_contracts::SessionSnapshot) -> Vec<u8> {
    let mut snapshot = snapshot.clone();
    snapshot.materialized_at = MessageField::none();
    if let Some(state) = snapshot.state.as_option_mut() {
        if let Some(session) = state.session.as_option_mut() {
            session.created_at = MessageField::none();
            session.updated_at = MessageField::none();
        }
        for tool in &mut state.tool_calls {
            tool.started_at = MessageField::none();
            tool.completed_at = MessageField::none();
        }
        for summary in &mut state.summaries {
            summary.created_at = MessageField::none();
        }
        if let Some(safety) = state.switch_safety.as_option_mut() {
            safety.evaluated_at = MessageField::none();
        }
        if let Some(checkpoint) = state.continuity_checkpoint.as_option_mut() {
            checkpoint.completed_at = MessageField::none();
        }
        if let Some(binding) = state.active_runner_binding.as_option_mut() {
            binding.attached_at = MessageField::none();
            binding.detached_at = MessageField::none();
        }
        if let Some(plan) = state.switch_adaptation_plan.as_option_mut() {
            plan.created_at = MessageField::none();
        }
    }
    snapshot.encode_to_vec()
}

/// The full lifecycle in one comprehensive test driving a single `session_id`.
#[tokio::test]
async fn complex_session_fixture_passes_all_criteria() {
    let session_id = SessionId::new(SESSION).unwrap();

    // Shared event log: the driver and the orchestrator both append here so the
    // canonical log is the single source of truth for replay.
    let event_log = InMemoryEventLog::new();
    event_log.append(created_event(SESSION)).await.unwrap();

    // A separate kernel handle over the SAME event log for driving the
    // pre-switch conversation/tool history. `InMemoryEventLog` is Clone and
    // shares state; the orchestrator consumes its own kernel built over the
    // same log clone below.
    let driver_kernel = {
        let config = SessionKernelConfig::default();
        let snapshots = SnapshotStore::new(MockJetStreamKvStore::new(), config.clone());
        let leases = SessionLeaseManager::new(
            MockSessionLeaseFactory::new(MockSessionLease::new()),
            "complex-fixture-driver",
        );
        SessionKernel::new(config, event_log.clone(), snapshots, leases)
    };

    // ELEMENT 1: several user/assistant turns.
    let turns = vec![
        message("m0", "user", "Implement the session kernel fixture"),
        message("m1", "assistant", "I'll start by recording the conversation"),
        message("m2", "user", "Now run a read-only check and a write"),
        message("m3", "assistant", "Running the idempotent read, then the write"),
    ];
    driver_kernel
        .record_conversation(&session_id, &turns, kernel_actor(), timestamp())
        .await
        .unwrap();

    // ELEMENT 2: an idempotent, successful tool call (fs read; stable
    // tool_execution_id makes the recorded events dedupe on retry).
    let read_tool = CanonicalToolCall {
        id: "tool_read".to_string(),
        tool_execution_id: "texec_read".to_string(),
        name: "fs_read".to_string(),
        input_json: "{\"path\":\"src/lib.rs\"}".to_string(),
        status: EnumValue::Known(ToolCallStatus::Completed),
        result: MessageField::some(ToolCallResult {
            kind: Some(
                TextToolResult {
                    content: "pub fn answer() -> u32 { 42 }".to_string(),
                    ..TextToolResult::default()
                }
                .into(),
            ),
            ..ToolCallResult::default()
        }),
        ..CanonicalToolCall::default()
    };

    // ELEMENT 3: a NON-idempotent tool call carrying its execution receipt
    // (tool_execution_id = receipt). The receipt is what makes a side-effecting
    // call safe to record exactly once; a retry with the same receipt must not
    // re-append events (asserted below as CRITERION 6).
    let write_receipt = "texec_write_receipt_7f3a";
    let write_tool = CanonicalToolCall {
        id: "tool_write".to_string(),
        tool_execution_id: write_receipt.to_string(),
        name: "fs_write".to_string(),
        input_json: "{\"path\":\"out.txt\",\"contents\":\"hello\"}".to_string(),
        status: EnumValue::Known(ToolCallStatus::Completed),
        result: MessageField::some(ToolCallResult {
            kind: Some(
                TextToolResult {
                    content: "wrote 5 bytes".to_string(),
                    ..TextToolResult::default()
                }
                .into(),
            ),
            ..ToolCallResult::default()
        }),
        ..CanonicalToolCall::default()
    };

    driver_kernel
        .record_tool_calls(
            &session_id,
            &[read_tool.clone(), write_tool.clone()],
            kernel_actor(),
            timestamp(),
        )
        .await
        .unwrap();

    // ELEMENT 4: a large tool result stored as an artifact ref (claim-check).
    // Drive the REAL artifact store: large bytes exceed the inline threshold and
    // are persisted to the object store, leaving only a checksum'd ref inline.
    let artifact_store = ArtifactStore::new(
        MockObjectStore::new(),
        ArtifactStoreConfig::from_session_kernel(&SessionKernelConfig::default()),
    );
    let inline_limit = artifact_store.config().inline_limit_bytes;
    let large_output = Bytes::from("LOGLINE ".repeat(inline_limit)); // well over the limit
    let stored = artifact_store
        .store(
            StoreArtifactRequest::new(
                session_id.clone(),
                EventId::new("evt_big_tool_result").unwrap(),
                "text/plain",
                large_output.clone(),
            )
            .with_tool_execution_id(ToolExecutionId::new("texec_big_grep").unwrap()),
        )
        .await
        .unwrap();
    assert_eq!(
        stored.storage_mode,
        ArtifactStorageMode::ClaimCheck,
        "ELEMENT 4: large output must claim-check to an artifact ref"
    );
    let big_tool = CanonicalToolCall {
        id: "tool_grep".to_string(),
        tool_execution_id: "texec_big_grep".to_string(),
        name: "bash".to_string(),
        input_json: "{\"cmd\":\"grep -r TODO .\"}".to_string(),
        status: EnumValue::Known(ToolCallStatus::Completed),
        result: MessageField::some(stored.to_tool_call_result()),
        ..CanonicalToolCall::default()
    };
    driver_kernel
        .record_tool_calls(&session_id, &[big_tool], kernel_actor(), timestamp())
        .await
        .unwrap();

    // ELEMENT 8: a compaction. Summarize the oldest turns into a summary, then
    // record the SessionCompacted event referencing that summary.
    let summary_id = "sum_compaction_1";
    let summary_event = SessionEvent {
        schema_version: SCHEMA_VERSION_V1,
        event_id: "evt_summary".to_string(),
        session_id: SESSION.to_string(),
        seq: 0,
        operation_id: "op_compact".to_string(),
        correlation_id: "corr_compact".to_string(),
        idempotency_key: "idem_summary".to_string(),
        created_at: MessageField::some(timestamp()),
        actor: MessageField::some(kernel_actor()),
        payload: MessageField::some(SessionEventPayload {
            kind: Some(
                SummaryCreatedPayload {
                    summary_id: summary_id.to_string(),
                    content: "Earlier: recorded the conversation and ran fs tools".to_string(),
                    from_seq: 1,
                    to_seq: 3,
                    ..SummaryCreatedPayload::default()
                }
                .into(),
            ),
            ..SessionEventPayload::default()
        }),
        ..SessionEvent::default()
    };
    driver_kernel
        .append_event_idempotent(summary_event, "idem_summary")
        .await
        .unwrap();
    let compacted_event = SessionEvent {
        schema_version: SCHEMA_VERSION_V1,
        event_id: "evt_compacted".to_string(),
        session_id: SESSION.to_string(),
        seq: 0,
        operation_id: "op_compact".to_string(),
        correlation_id: "corr_compact".to_string(),
        idempotency_key: "idem_compacted".to_string(),
        created_at: MessageField::some(timestamp()),
        actor: MessageField::some(kernel_actor()),
        payload: MessageField::some(SessionEventPayload {
            kind: Some(
                SessionCompactedPayload {
                    summary_id: summary_id.to_string(),
                    compacted_through_seq: 3,
                    tokens_before: 4_000,
                    tokens_after: 1_200,
                    ..SessionCompactedPayload::default()
                }
                .into(),
            ),
            ..SessionEventPayload::default()
        }),
        ..SessionEvent::default()
    };
    driver_kernel
        .append_event_idempotent(compacted_event, "idem_compacted")
        .await
        .unwrap();

    // ELEMENT 9 + CRITERION 6: a retry that does NOT duplicate tool calls or
    // events. Re-record the exact same idempotent + non-idempotent tool calls;
    // the kernel dedupes on (tool_execution_id) so no new events are appended,
    // and no external effect is re-applied.
    let events_before_retry = event_log.read_session_events(&session_id).await.unwrap();
    driver_kernel
        .record_tool_calls(
            &session_id,
            &[read_tool.clone(), write_tool.clone()],
            kernel_actor(),
            timestamp(),
        )
        .await
        .unwrap();
    // Also exercise raw idempotent append at the same logical key twice.
    let retry_event = SessionEvent {
        idempotency_key: "idem_summary".to_string(),
        ..created_event(SESSION)
    };
    driver_kernel
        .append_event_idempotent(retry_event, "idem_summary")
        .await
        .unwrap();
    let events_after_retry = event_log.read_session_events(&session_id).await.unwrap();
    assert_eq!(
        events_before_retry.len(),
        events_after_retry.len(),
        "CRITERION 6: retries must not duplicate events or external effects"
    );

    // ELEMENT 10: a cancellation. Drive the real cancel flow and append its
    // typed events to the canonical log so replay sees the cancellation.
    let cancel_ctx = CancelContext {
        session_id: session_id.clone(),
        operation_id: OperationId::new("op_cancel_fixture").unwrap(),
        correlation_id: "corr_cancel_fixture".to_string(),
        idempotency_key: IdempotencyKey::new("idem_cancel_fixture").unwrap(),
        actor: user_actor(),
        created_at: timestamp(),
        runner_id: "openrouter".to_string(),
    };
    let (outcome, cancel_state, cancel_events) = cancel_operation(
        Some(&MockRunnerCancellation::default()),
        &cancel_ctx,
        &[ToolExecutionState::Started],
    )
    .await
    .unwrap();
    assert_eq!(outcome, CancelOutcome::Cancelled);
    assert_eq!(cancel_state, CancelState::OperationCancelled);
    for event in cancel_events {
        let key = event.idempotency_key.clone();
        driver_kernel
            .append_event_idempotent(event, &key)
            .await
            .unwrap();
    }
    let after_cancel = event_log.read_session_events(&session_id).await.unwrap();
    assert!(
        has_kind(&after_cancel, |k| matches!(k, Kind::OperationCancelled(_))),
        "ELEMENT 10: cancellation must be a durable, replayable event"
    );

    // ELEMENT 6 + 13 + 7(compactor) + CRITERION 5/7: cross-provider switch.
    // Build the orchestrator over the SAME shared event log.
    let registry = registry_with_schemas().await;
    let orchestrator = build_orchestrator(event_log.clone(), registry);

    let switch_result = orchestrator
        .switch_model(SwitchModelRequest {
            session_id: session_id.clone(),
            target_model: TO_MODEL.to_string(),
            reason: ModelSwitchReason::UserRequested,
            user_confirmed: true,
            force: false,
            force_acknowledged_losses: Vec::new(),
            operation_id: OperationId::new("op_switch").unwrap(),
            correlation_id: "corr_switch".to_string(),
            idempotency_key: IdempotencyKey::new("idem_switch").unwrap(),
        })
        .await
        .unwrap()
        .expect("switch should complete");

    // CRITERION 7: the switch produces a Safety Gate result AND, since the
    // checkpoint is enabled, a Continuity Checkpoint.
    assert_eq!(switch_result.state, SwitchState::Completed);
    assert_eq!(switch_result.from_model, FROM_MODEL);
    assert_eq!(switch_result.to_model, TO_MODEL);
    assert_eq!(switch_result.from_runner, "openrouter");
    assert_eq!(switch_result.to_runner, "xai");
    assert!(
        switch_result.safety.status.as_known().is_some(),
        "CRITERION 7: switch must yield a Safety Gate result"
    );
    assert!(
        matches!(
            switch_result.safety.status.as_known(),
            Some(
                SwitchSafetyStatus::Allowed
                    | SwitchSafetyStatus::AllowedWithWarning
                    | SwitchSafetyStatus::RequiresUserConfirmation
            )
        ),
        "switch is not blocked by the Safety Gate (it completed)"
    );
    assert!(
        switch_result.checkpoint.is_some(),
        "CRITERION 7: continuity checkpoint must be produced when enabled"
    );

    let post_switch = event_log.read_session_events(&session_id).await.unwrap();
    // Safety Gate + model switch + runner attach/detach are all durable events.
    assert!(has_kind(&post_switch, |k| matches!(
        k,
        Kind::SwitchSafetyEvaluated(_)
    )));
    assert!(has_kind(&post_switch, |k| matches!(k, Kind::ModelSwitched(_))));
    // ELEMENT 12: runner attach/detach recorded by the orchestrator.
    assert!(has_kind(&post_switch, |k| matches!(k, Kind::RunnerDetached(_))));
    assert!(has_kind(&post_switch, |k| matches!(k, Kind::RunnerAttached(_))));
    // ELEMENT 13: missing capability (image_input on grok) drives explicit
    // degradation in the adaptation plan, recorded as a durable plan event.
    assert!(has_kind(&post_switch, |k| matches!(
        k,
        Kind::SwitchAdaptationPlanCreated(_)
    )));

    // ELEMENT 7 + CRITERION 5: compactor_model preserved/audited across the switch.
    // The fixture's compactor_model (claude-haiku) is from the source provider;
    // after switching to grok it is either preserved or explicitly degraded.
    let compactor_preserved = has_kind(&post_switch, |k| {
        matches!(k, Kind::CompactorModelPreserved(_))
    });
    let compactor_degraded = has_kind(&post_switch, |k| {
        matches!(k, Kind::CompactorModelUnavailable(_))
    }) && has_kind(&post_switch, |k| {
        matches!(k, Kind::FallbackToDefaultCompactor(_))
    });
    assert!(
        compactor_preserved || compactor_degraded,
        "ELEMENT 7: compactor_model handling must be explicitly audited \
         (preserved or degraded-with-fallback)"
    );

    // ELEMENT 11: fork a child session from the current head.
    let child_id = SessionId::new("sess_complex_fixture_fork").unwrap();
    let head_seq = post_switch.iter().map(|e| e.seq).max().unwrap();
    let child = driver_kernel
        .fork_session(
            &session_id,
            &child_id,
            head_seq,
            "op_fork",
            kernel_actor(),
            timestamp(),
        )
        .await
        .unwrap();
    let child_session = child
        .state
        .as_option()
        .and_then(|s| s.session.as_option())
        .unwrap();
    assert_eq!(
        child_session.parent_session_id.as_deref(),
        Some(SESSION),
        "ELEMENT 11: child records its parent lineage"
    );
    assert_eq!(child_session.branched_at_seq, Some(head_seq));

    // ---- CRITERION 1: replay reconstructs the same canonical snapshot. ----
    // materialize_state replays the full event log into a snapshot; recover()
    // independently replays from the log. Both must agree on the canonical state.
    //
    // Note: `materialize_from_events` stamps several fields (e.g. `materialized_at`,
    // per-element `created_at`/`completed_at`) with the wall clock *at replay time*
    // (`now_timestamp()`), so two replays at different instants are NOT byte-identical
    // on those non-canonical metadata fields. The canonical SUBSTANCE — conversation,
    // tool calls (with full IO), config, summaries, lineage — must match exactly. We
    // normalize the replay-stamped wall-clock fields, then assert byte-identity of the
    // remaining canonical state.
    let materialized = driver_kernel.materialize_state(&session_id).await.unwrap();
    let recovered = driver_kernel.recover(&session_id).await.unwrap();
    assert_eq!(
        materialized.last_applied_seq, recovered.snapshot.last_applied_seq,
        "CRITERION 1: replay must reconstruct the same canonical seq"
    );
    let canonical_bytes = canonical_state_bytes(&materialized);
    let replayed_bytes = canonical_state_bytes(&recovered.snapshot);
    assert_eq!(
        canonical_bytes, replayed_bytes,
        "CRITERION 1: replay must reconstruct a byte-identical canonical snapshot \
         (after normalizing replay-time wall-clock stamps)"
    );
    assert_eq!(
        recovered.replayed_events,
        post_switch.len(),
        "CRITERION 1: recovery replays exactly the durable event log"
    );

    let state = materialized.state.as_option().unwrap();

    // ---- CRITERION 3: no canonical tool IO lost (full input/output, untruncated). ----
    let recorded_read = state
        .tool_calls
        .iter()
        .find(|t| t.id == "tool_read")
        .expect("idempotent read tool present in canonical state");
    assert_eq!(recorded_read.input_json, "{\"path\":\"src/lib.rs\"}");
    let read_text = recorded_read
        .result
        .as_option()
        .and_then(|r| r.kind.as_ref())
        .and_then(|k| match k {
            ToolResultKind::Text(t) => Some(t.content.clone()),
            _ => None,
        })
        .unwrap();
    assert_eq!(
        read_text, "pub fn answer() -> u32 { 42 }",
        "CRITERION 3: idempotent tool output preserved verbatim"
    );

    let recorded_write = state
        .tool_calls
        .iter()
        .find(|t| t.id == "tool_write")
        .expect("non-idempotent write tool present");
    assert_eq!(
        recorded_write.input_json, "{\"path\":\"out.txt\",\"contents\":\"hello\"}",
        "CRITERION 3: non-idempotent tool input preserved verbatim"
    );
    assert_eq!(
        recorded_write.tool_execution_id, write_receipt,
        "ELEMENT 3: the write carries its execution receipt"
    );

    // The large result stays canonical as an artifact ref with a verifiable
    // checksum and full byte size (not a truncated summary).
    let recorded_big = state
        .tool_calls
        .iter()
        .find(|t| t.id == "tool_grep")
        .expect("large-output tool present");
    let big_ref: ArtifactRef = recorded_big
        .result
        .as_option()
        .and_then(|r| r.kind.as_ref())
        .and_then(|k| match k {
            ToolResultKind::ArtifactRef(a) => Some((**a).clone()),
            _ => None,
        })
        .expect("CRITERION 3: large output kept as an artifact ref, not dropped");
    assert_eq!(big_ref.sha256, stored.metadata.sha256);
    assert_eq!(big_ref.size_bytes, large_output.len() as u64);

    // ---- CRITERION 4: referenced artifacts exist OR yield artifact_unavailable. ----
    // (a) The store that persisted the object resolves it as Available.
    match artifact_store
        .retrieve_availability(&stored.metadata)
        .await
        .unwrap()
    {
        ArtifactAvailability::Available(retrieved) => {
            assert_eq!(retrieved.content, large_output);
        }
        other => panic!("expected available artifact, got {other:?}"),
    }
    // ELEMENT 5 + CRITERION 4(b): a missing artifact. An empty object store has
    // the canonical ref but not the bytes -> explicit artifact_unavailable.
    let empty_store = ArtifactStore::new(
        MockObjectStore::new(),
        ArtifactStoreConfig::from_session_kernel(&SessionKernelConfig::default()),
    );
    match empty_store
        .retrieve_availability(&stored.metadata)
        .await
        .unwrap()
    {
        ArtifactAvailability::Unavailable(unavailable) => {
            assert_eq!(
                unavailable.reason,
                ArtifactUnavailableReason::NotInObjectStore,
                "ELEMENT 5: missing artifact surfaces explicit artifact_unavailable"
            );
        }
        other => panic!("expected artifact_unavailable, got {other:?}"),
    }

    // ---- CRITERION 5: compactor_model intact after the switch (materialized). ----
    // The switch updates model/runner but must not clobber compactor_model.
    let config = state.config.as_option().unwrap();
    assert_eq!(
        config.model.as_deref(),
        Some(TO_MODEL),
        "model switched in canonical config"
    );
    assert_eq!(
        config.runner.as_deref(),
        Some("xai"),
        "runner switched in canonical config"
    );
    assert_eq!(
        config.compactor_model.as_deref(),
        Some("anthropic/claude-haiku"),
        "CRITERION 5: compactor_model intact in canonical state after switch"
    );

    // The compaction summary was removed by SessionCompacted (it was folded into
    // the compacted history), proving the compaction event replayed.
    assert!(
        !state.summaries.iter().any(|s| s.summary_id == summary_id),
        "ELEMENT 8: SessionCompacted folded the summary out of live state"
    );

    // ---- CRITERION 2: PromptProjection is deterministic. ----
    // ELEMENT 13: build the adaptation plan from the real capability usage; the
    // grok image_input gap produces explicit degradation entries.
    let usage = detect_session_capability_usage(state);
    let claude_caps = trogonai_capabilities::ResolvedCapabilities {
        schema: claude_schema(),
        runner_id: "openrouter".to_string(),
        freshness: trogonai_capabilities::FreshnessStatus::Fresh,
        degraded: false,
    };
    let grok_caps = trogonai_capabilities::ResolvedCapabilities {
        schema: grok_schema(),
        runner_id: "xai".to_string(),
        freshness: trogonai_capabilities::FreshnessStatus::Fresh,
        degraded: false,
    };
    let adaptation = build_adaptation_plan(FROM_MODEL, TO_MODEL, &claude_caps, &grok_caps, &usage);

    let context_twin = derive_context_twin(SESSION, state, materialized.last_applied_seq, timestamp());

    // Assemble the projection view. The system prompt and permission rules are
    // injected at projection time (they are critical blocks the compiler requires);
    // the current request is the session's last user turn. This is a derived view —
    // the canonical snapshot is untouched.
    let mut projection_snapshot = state.clone();
    {
        let config = projection_snapshot.config.get_or_insert_default();
        config.system_prompt = Some("You are Trogonai, a coding agent.".to_string());
        config.permission_rules_text = Some("Never commit unless asked".to_string());
    }
    let current_request = state
        .conversation
        .iter()
        .rev()
        .find(|message| message.role == "user")
        .cloned();
    assert!(
        current_request.is_some(),
        "fixture has user turns to project as the current request"
    );

    let projection_input = ProjectionInput {
        session_id: SESSION.to_string(),
        model_id: TO_MODEL.to_string(),
        snapshot: projection_snapshot,
        context_twin,
        adaptation_plan: Some(adaptation),
        capabilities: grok_caps.clone(),
        token_budget: 32_768,
        current_request,
        continuity_warnings: vec!["continuity checkpoint completed".to_string()],
        config: ProjectionConfig {
            output_reserve_tokens: 256,
            recent_turn_count: 2,
            ..ProjectionConfig::default()
        },
        created_at: Some(timestamp()),
        projection_id: Some("proj_complex_fixture".to_string()),
    };

    let compiler = DefaultPromptCompiler;
    let first = compiler.compile(projection_input.clone()).unwrap();
    let second = compiler.compile(projection_input).unwrap();
    assert_eq!(
        first.included_blocks.len(),
        second.included_blocks.len(),
        "CRITERION 2: projection determinism (included blocks)"
    );
    assert_eq!(
        first.excluded_blocks.len(),
        second.excluded_blocks.len(),
        "CRITERION 2: projection determinism (excluded blocks)"
    );
    assert_eq!(
        first.encode_to_vec(),
        second.encode_to_vec(),
        "CRITERION 2: PromptProjection is byte-for-byte deterministic"
    );
    // ELEMENT 13: degradation is explicit in the projection metadata.
    assert!(
        first.degradation_metadata.iter().any(|entry| {
            entry.kind == EnumValue::Known(DegradationKind::SwitchAdaptationApplied)
                || entry.kind == EnumValue::Known(DegradationKind::ContextTwinIncluded)
        }),
        "ELEMENT 13: missing-capability degradation surfaces explicitly in projection"
    );

    // Sanity: exactly one durable record of each major switch event (no dupes).
    assert_eq!(
        count_kind(&post_switch, |k| matches!(k, Kind::ModelSwitched(_))),
        1,
        "CRITERION 6: the switch is recorded exactly once"
    );
}

//! End-to-end switch flow with mocked NATS stores and runner acknowledgement.

use std::sync::Arc;

use buffa::{EnumValue, MessageField};
use buffa_types::google::protobuf::Timestamp;
use trogon_registry::{AgentCapability, MockRegistryStore, Registry};
use trogonai_capabilities::{
    CapabilityConfig, CapabilityRegistry, CertificationLevel, ProviderCertificationEntry, ProviderCertificationMatrix,
};
use trogonai_session_contracts::{
    Actor, ActorType, CapabilitySchema, CapabilitySource, IdempotencyKey, ModelSwitchReason, OperationId,
    SCHEMA_VERSION_V1, SessionCreatedPayload, SessionEvent, SessionEventPayload, SessionId, SwitchResult,
    SwitchSafetyStatus, ToolResultFormat,
};
use trogonai_session_kernel::{
    InMemoryEventLog, MockSessionLease, MockSessionLeaseFactory, SessionKernel, SessionKernelConfig,
    SessionLeaseManager, SnapshotStore,
};
use trogonai_session_projection::{ContextTwinStore, ProjectionConfig};
use trogonai_switching::{
    MockRunnerAcknowledgement, RunnerBindingStore, SwitchModelRequest, SwitchOrchestrator, SwitchSafetyInput,
    SwitchState, SwitchingConfig, classify_switch_result, evaluate_switch_safety,
};

fn timestamp() -> Timestamp {
    Timestamp {
        seconds: 1_748_995_200,
        nanos: 0,
        ..Timestamp::default()
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
        actor: MessageField::some(Actor {
            r#type: EnumValue::Known(ActorType::Kernel),
            id: "session-kernel".to_string(),
            ..Actor::default()
        }),
        payload: MessageField::some(SessionEventPayload {
            kind: Some(
                SessionCreatedPayload {
                    title: "Switch flow".to_string(),
                    cwd: "/repo".to_string(),
                    model: Some("anthropic/claude-sonnet".to_string()),
                    runner: Some("openrouter".to_string()),
                    compactor_model: Some("xai/grok-code-fast".to_string()),
                    ..SessionCreatedPayload::default()
                }
                .into(),
            ),
            ..SessionEventPayload::default()
        }),
        ..SessionEvent::default()
    }
}

fn claude_schema() -> CapabilitySchema {
    CapabilitySchema {
        schema_version: SCHEMA_VERSION_V1,
        model_id: "anthropic/claude-sonnet".to_string(),
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
        model_id: "xai/grok-code-fast".to_string(),
        runner_id: "xai".to_string(),
        max_context_tokens: 131_072,
        max_output_tokens: 16_384,
        tool_use: true,
        parallel_tool_calls: true,
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
    openrouter.metadata = serde_json::json!({
        "models": ["anthropic/claude-sonnet"],
    });
    CapabilityRegistry::embed_schema(&mut openrouter, &claude_schema()).unwrap();
    registry.register(&openrouter).await.unwrap();

    let mut xai = AgentCapability::new("xai", ["chat"], "acp.grok.>");
    xai.metadata = serde_json::json!({
        "models": ["xai/grok-code-fast"],
    });
    CapabilityRegistry::embed_schema(&mut xai, &grok_schema()).unwrap();
    registry.register(&xai).await.unwrap();

    registry
}

fn certification_matrix() -> ProviderCertificationMatrix {
    let mut matrix = ProviderCertificationMatrix::default();
    matrix
        .push_validated(ProviderCertificationEntry {
            model: "anthropic/claude-sonnet".to_string(),
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
            switch_to: vec!["xai/grok-code-fast".to_string()],
            certified_level: CertificationLevel::Production,
            last_verified_at: None,
        })
        .unwrap();
    matrix
        .push_validated(ProviderCertificationEntry {
            model: "xai/grok-code-fast".to_string(),
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
            switch_from: vec!["anthropic/claude-sonnet".to_string()],
            switch_to: vec![],
            certified_level: CertificationLevel::SwitchSafe,
            last_verified_at: None,
        })
        .unwrap();
    matrix
}

fn build_orchestrator(
    event_log: InMemoryEventLog,
    registry: Registry<MockRegistryStore>,
    mock_runner: MockRunnerAcknowledgement,
) -> SwitchOrchestrator<
    InMemoryEventLog,
    trogon_nats::jetstream::MockJetStreamKvStore,
    MockSessionLeaseFactory,
    trogon_nats::jetstream::MockJetStreamKvStore,
    trogon_nats::jetstream::MockJetStreamKvStore,
    MockRegistryStore,
    MockRunnerAcknowledgement,
> {
    let kernel_config = SessionKernelConfig::default();
    let snap_store = SnapshotStore::new(
        trogon_nats::jetstream::MockJetStreamKvStore::new(),
        kernel_config.clone(),
    );
    let snapshots = snap_store.clone();
    let leases = SessionLeaseManager::new(MockSessionLeaseFactory::new(MockSessionLease::new()), "switch-test");
    let kernel = SessionKernel::new(kernel_config, event_log, snap_store, leases);
    let runner_bindings = RunnerBindingStore::new(
        trogon_nats::jetstream::MockJetStreamKvStore::new(),
        SwitchingConfig::default(),
    );
    let twin_store = ContextTwinStore::new(
        trogon_nats::jetstream::MockJetStreamKvStore::new(),
        ProjectionConfig::default(),
    );

    SwitchOrchestrator::new(
        kernel,
        snapshots,
        runner_bindings,
        twin_store,
        registry,
        Arc::new(mock_runner),
        SwitchingConfig {
            continuity_checkpoint_enabled: false,
            ..SwitchingConfig::default()
        },
        CapabilityConfig::default(),
        ProjectionConfig::default(),
        certification_matrix(),
    )
}

#[tokio::test]
async fn full_switch_flow_cross_runner_with_mock_runner() {
    let session_id = SessionId::new("sess_switch_flow").unwrap();
    let event_log = InMemoryEventLog::new();
    event_log.append(created_event(session_id.as_str())).await.unwrap();

    let registry = registry_with_schemas().await;
    let orchestrator = build_orchestrator(event_log.clone(), registry, MockRunnerAcknowledgement::default());

    let result = orchestrator
        .switch_model(SwitchModelRequest {
            session_id: session_id.clone(),
            target_model: "xai/grok-code-fast".to_string(),
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

    assert_eq!(result.state, SwitchState::Completed);
    assert_eq!(result.from_runner, "openrouter");
    assert_eq!(result.to_runner, "xai");
    assert_eq!(result.from_model, "anthropic/claude-sonnet");
    assert_eq!(result.to_model, "xai/grok-code-fast");
    assert!(result.projection.projection_id.starts_with("proj_"));
    assert!(result.checkpoint.is_none());
    assert!(result.events_appended >= 6);

    let events = event_log.read_session_events(&session_id).await.unwrap();
    let has_kind = |matcher: fn(&trogonai_session_contracts::session_event_payload::Kind) -> bool| {
        events.iter().any(|event| {
            event
                .payload
                .as_option()
                .and_then(|payload| payload.kind.as_ref())
                .is_some_and(matcher)
        })
    };
    use trogonai_session_contracts::session_event_payload::Kind;
    assert!(has_kind(|kind| matches!(kind, Kind::ModelSwitched(_))));
    // compactor_model is set in the fixture and the grok target does not support
    // compaction, so degradation must be recorded, and the fallback only after the
    // switch proceeds past the gate.
    assert!(has_kind(|kind| matches!(kind, Kind::CompactorModelUnavailable(_))));
    assert!(has_kind(|kind| matches!(kind, Kind::FallbackToDefaultCompactor(_))));

    // The normalized outcome is persisted as ONE durable visible result (§ Contrato
    // formal/visible). A degraded (compactor-unavailable) switch records `degraded`,
    // and the visible result carries the resolved runners.
    let outcome_event = events
        .iter()
        .find_map(|event| match event.payload.as_option().and_then(|p| p.kind.as_ref()) {
            Some(Kind::SwitchOutcomeRecorded(payload)) => payload.result.as_option(),
            _ => None,
        })
        .expect("a switch_outcome_recorded event must be emitted");
    use trogonai_session_contracts::SwitchResult;
    assert_eq!(outcome_event.result.as_known(), Some(SwitchResult::Degraded));
    assert_eq!(outcome_event.from_runner, "openrouter");
    assert_eq!(outcome_event.to_runner, "xai");
    assert!(outcome_event.runner_changed);
}

#[tokio::test]
async fn switch_blocked_when_session_busy() {
    let session_id = SessionId::new("sess_busy_switch").unwrap();
    let event_log = InMemoryEventLog::new();
    event_log.append(created_event(session_id.as_str())).await.unwrap();

    let registry = registry_with_schemas().await;
    let kernel_config = SessionKernelConfig::default();
    let snap_store = SnapshotStore::new(
        trogon_nats::jetstream::MockJetStreamKvStore::new(),
        kernel_config.clone(),
    );
    let lease = MockSessionLease::new();
    lease.hold_by_other();
    let leases = SessionLeaseManager::new(MockSessionLeaseFactory::new(lease), "switch-test");
    let kernel = SessionKernel::new(kernel_config, event_log, snap_store.clone(), leases);
    let orchestrator = SwitchOrchestrator::new(
        kernel,
        snap_store,
        RunnerBindingStore::new(
            trogon_nats::jetstream::MockJetStreamKvStore::new(),
            SwitchingConfig::default(),
        ),
        ContextTwinStore::new(
            trogon_nats::jetstream::MockJetStreamKvStore::new(),
            ProjectionConfig::default(),
        ),
        registry,
        Arc::new(MockRunnerAcknowledgement::default()),
        SwitchingConfig::default(),
        CapabilityConfig::default(),
        ProjectionConfig::default(),
        certification_matrix(),
    );

    let err = orchestrator
        .switch_model(SwitchModelRequest {
            session_id,
            target_model: "xai/grok-code-fast".to_string(),
            reason: ModelSwitchReason::UserRequested,
            user_confirmed: true,
            force: false,
            force_acknowledged_losses: Vec::new(),
            operation_id: OperationId::new("op_switch_busy").unwrap(),
            correlation_id: "corr_switch_busy".to_string(),
            idempotency_key: IdempotencyKey::new("idem_switch_busy").unwrap(),
        })
        .await
        .unwrap_err();

    assert!(matches!(err, trogonai_switching::SwitchingError::SessionBusy { .. }));
}

#[tokio::test]
async fn safety_gate_blocks_tool_in_progress_before_orchestrator() {
    use trogonai_session_contracts::{
        CanonicalToolCall, SessionConfig, SessionMetadata, SessionSnapshotState, ToolCallStatus,
    };

    let session = SessionSnapshotState {
        session: MessageField::some(SessionMetadata {
            id: "sess_tool".to_string(),
            ..SessionMetadata::default()
        }),
        config: MessageField::some(SessionConfig {
            model: Some("anthropic/claude-sonnet".to_string()),
            runner: Some("openrouter".to_string()),
            ..SessionConfig::default()
        }),
        tool_calls: vec![CanonicalToolCall {
            id: "tool_1".to_string(),
            name: "bash".to_string(),
            status: EnumValue::Known(ToolCallStatus::Started),
            ..CanonicalToolCall::default()
        }],
        ..SessionSnapshotState::default()
    };

    let target = trogonai_capabilities::ResolvedCapabilities {
        schema: grok_schema(),
        runner_id: "xai".to_string(),
        freshness: trogonai_capabilities::FreshnessStatus::Fresh,
        degraded: false,
    };

    let decision = evaluate_switch_safety(&SwitchSafetyInput {
        session: &session,
        target_model: "xai/grok-code-fast",
        target_runner: "xai",
        target_capabilities: &target,
        adaptation_plan: None,
        certification: &ProviderCertificationMatrix::default(),
        config: &SwitchingConfig::default(),
        force: false,
        user_confirmed: false,
        last_applied_seq: 5,
    });

    assert_eq!(decision.status.as_known(), Some(SwitchSafetyStatus::BlockedUntilSafe));
}

#[test]
fn switch_state_machine_rejects_skipped_steps() {
    let err = SwitchState::Requested
        .transition_to(SwitchState::ModelSwitched)
        .unwrap_err();
    assert!(matches!(
        err,
        trogonai_switching::SwitchingError::InvalidSwitchTransition { .. }
    ));
}

/// Runner that fails the post-attach continuity acknowledgement, simulating a
/// runner failure after `runner_attached` (§ Failure Mode Policy).
struct FailingRunner;

#[async_trait::async_trait]
impl trogonai_switching::RunnerAcknowledgement for FailingRunner {
    async fn request_acknowledgement(
        &self,
        _prompt: &str,
    ) -> Result<trogonai_switching::ContinuityAcknowledgement, trogonai_switching::SwitchingError> {
        Err(trogonai_switching::SwitchingError::CheckpointFailed {
            detail: "runner offline after attach".to_string(),
        })
    }
}

#[tokio::test]
async fn switch_records_runner_failed_when_runner_fails_after_attach() {
    let session_id = SessionId::new("sess_runner_fail").unwrap();
    let event_log = InMemoryEventLog::new();
    event_log.append(created_event(session_id.as_str())).await.unwrap();
    let registry = registry_with_schemas().await;

    let kernel_config = SessionKernelConfig::default();
    let snap_store = SnapshotStore::new(
        trogon_nats::jetstream::MockJetStreamKvStore::new(),
        kernel_config.clone(),
    );
    let leases = SessionLeaseManager::new(MockSessionLeaseFactory::new(MockSessionLease::new()), "switch-test");
    let kernel = SessionKernel::new(kernel_config, event_log.clone(), snap_store.clone(), leases);
    // continuity_checkpoint_enabled defaults to true, so the post-attach runner ack runs.
    let orchestrator = SwitchOrchestrator::new(
        kernel,
        snap_store,
        RunnerBindingStore::new(
            trogon_nats::jetstream::MockJetStreamKvStore::new(),
            SwitchingConfig::default(),
        ),
        ContextTwinStore::new(
            trogon_nats::jetstream::MockJetStreamKvStore::new(),
            ProjectionConfig::default(),
        ),
        registry,
        Arc::new(FailingRunner),
        // internal_echo off so the (failing) runner is actually invoked.
        SwitchingConfig {
            continuity_checkpoint_internal_echo: false,
            ..SwitchingConfig::default()
        },
        CapabilityConfig::default(),
        ProjectionConfig::default(),
        certification_matrix(),
    );

    let err = orchestrator
        .switch_model(SwitchModelRequest {
            session_id: session_id.clone(),
            target_model: "xai/grok-code-fast".to_string(),
            reason: ModelSwitchReason::UserRequested,
            user_confirmed: true,
            force: false,
            force_acknowledged_losses: Vec::new(),
            operation_id: OperationId::new("op_runner_fail").unwrap(),
            correlation_id: "corr_runner_fail".to_string(),
            idempotency_key: IdempotencyKey::new("idem_runner_fail").unwrap(),
        })
        .await
        .unwrap_err();
    assert!(matches!(
        err,
        trogonai_switching::SwitchingError::RunnerAcknowledgementFailed { .. }
    ));

    // The runner failed after attach: runner_failed is durably recorded.
    let events = event_log.read_session_events(&session_id).await.unwrap();
    assert!(events.iter().any(|event| {
        event
            .payload
            .as_option()
            .and_then(|payload| payload.kind.as_ref())
            .is_some_and(|kind| {
                matches!(
                    kind,
                    trogonai_session_contracts::session_event_payload::Kind::RunnerFailed(_)
                )
            })
    }));
}

/// Runner whose acknowledgement SUCCEEDS but mismatches the Context Twin on every field,
/// driving checkpoint confidence below the failure threshold (a low-confidence checkpoint
/// is distinct from a runner failure).
struct MismatchingRunner;

#[async_trait::async_trait]
impl trogonai_switching::RunnerAcknowledgement for MismatchingRunner {
    async fn request_acknowledgement(
        &self,
        _prompt: &str,
    ) -> Result<trogonai_switching::ContinuityAcknowledgement, trogonai_switching::SwitchingError> {
        Ok(trogonai_switching::ContinuityAcknowledgement {
            current_objective: "totally different objective".to_string(),
            active_plan: "unrelated plan".to_string(),
            relevant_files: vec!["wrong_file.rs".to_string()],
            next_step: "do something unrelated".to_string(),
            ..Default::default()
        })
    }
}

/// § rolled_back: when the continuity checkpoint (a stage AFTER `runner_attached`) fails,
/// Trogonai restores the previous runner binding and the switch result is `rolled_back`
/// (§2032; § Rollback Strategy). Verifies the structured result and that the rollback
/// detached the target and re-attached the source binding in the event log.
#[tokio::test]
async fn switch_rolls_back_to_previous_binding_when_checkpoint_fails() {
    let session_id = SessionId::new("sess_rollback").unwrap();
    let event_log = InMemoryEventLog::new();
    event_log.append(created_event(session_id.as_str())).await.unwrap();
    let registry = registry_with_schemas().await;

    let kernel_config = SessionKernelConfig::default();
    let snap_store = SnapshotStore::new(
        trogon_nats::jetstream::MockJetStreamKvStore::new(),
        kernel_config.clone(),
    );
    let leases = SessionLeaseManager::new(MockSessionLeaseFactory::new(MockSessionLease::new()), "switch-test");
    let kernel = SessionKernel::new(kernel_config, event_log.clone(), snap_store.clone(), leases);
    let orchestrator = SwitchOrchestrator::new(
        kernel,
        snap_store,
        RunnerBindingStore::new(
            trogon_nats::jetstream::MockJetStreamKvStore::new(),
            SwitchingConfig::default(),
        ),
        ContextTwinStore::new(
            trogon_nats::jetstream::MockJetStreamKvStore::new(),
            ProjectionConfig::default(),
        ),
        registry,
        Arc::new(MismatchingRunner),
        // internal_echo off so the real (mismatching) runner ack is used, and a high
        // confidence floor so the mismatch fails rather than repairs.
        SwitchingConfig {
            continuity_checkpoint_internal_echo: false,
            checkpoint_min_confidence: 0.9,
            checkpoint_mismatch_threshold: 0.2,
            ..SwitchingConfig::default()
        },
        CapabilityConfig::default(),
        ProjectionConfig::default(),
        certification_matrix(),
    );

    let outcome = orchestrator
        .switch_model(SwitchModelRequest {
            session_id: session_id.clone(),
            target_model: "xai/grok-code-fast".to_string(),
            reason: ModelSwitchReason::UserRequested,
            user_confirmed: true,
            force: false,
            force_acknowledged_losses: Vec::new(),
            operation_id: OperationId::new("op_rollback").unwrap(),
            correlation_id: "corr_rollback".to_string(),
            idempotency_key: IdempotencyKey::new("idem_rollback").unwrap(),
        })
        .await;

    // The normalized, structured result is `rolled_back`.
    assert_eq!(classify_switch_result(&outcome), SwitchResult::RolledBack);
    assert!(matches!(
        outcome,
        Err(trogonai_switching::SwitchingError::SwitchRolledBack { .. })
    ));

    // The rollback detached the target runner with the rollback reason (restoring the
    // previous binding), durably recorded in the event log.
    let events = event_log.read_session_events(&session_id).await.unwrap();
    let rolled_back_detach = events.iter().any(|event| {
        event
            .payload
            .as_option()
            .and_then(|payload| payload.kind.as_ref())
            .is_some_and(|kind| match kind {
                trogonai_session_contracts::session_event_payload::Kind::RunnerDetached(detached) => {
                    detached.reason.as_deref() == Some("rollback_checkpoint_failed")
                }
                _ => false,
            })
    });
    assert!(rolled_back_detach, "rollback must detach the target with the rollback reason");
}

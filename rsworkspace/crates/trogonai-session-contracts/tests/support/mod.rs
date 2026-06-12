#![allow(dead_code)]

use buffa::{EnumValue, Message as _, MessageField};
use buffa_types::google::protobuf::Timestamp;
use trogonai_session_contracts::{
    Actor, ActorType, ArtifactMetadata, ArtifactRetentionPolicy,
    CapabilitySchema, CapabilitySource, ContextTwin, EncryptionStatus, ModelSwitchedPayload,
    ModelSwitchReason, RunnerBinding, RunnerBindingStatus, SCHEMA_VERSION_V1, SessionCreatedPayload,
    SessionEvent, SessionEventPayload, SessionSnapshot, SessionSnapshotState, ToolResultFormat,
};

pub fn sample_timestamp() -> Timestamp {
    Timestamp {
        seconds: 1_748_995_200,
        nanos: 0,
        ..Timestamp::default()
    }
}

pub fn sample_session_event() -> SessionEvent {
    SessionEvent {
        schema_version: SCHEMA_VERSION_V1,
        event_id: "evt_golden_session_created".to_string(),
        session_id: "sess_golden".to_string(),
        seq: 1,
        operation_id: "op_golden_create".to_string(),
        correlation_id: "corr_golden_create".to_string(),
        causation_id: None,
        idempotency_key: "idem_golden_create".to_string(),
        created_at: MessageField::some(sample_timestamp()),
        actor: MessageField::some(Actor {
            r#type: EnumValue::Known(ActorType::Kernel),
            id: "session-kernel".to_string(),
            ..Actor::default()
        }),
        payload: MessageField::some(SessionEventPayload {
            kind: Some(
                SessionCreatedPayload {
                    title: "Golden fixture session".to_string(),
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

pub fn sample_model_switched_event() -> SessionEvent {
    SessionEvent {
        schema_version: SCHEMA_VERSION_V1,
        event_id: "evt_golden_model_switched".to_string(),
        session_id: "sess_golden".to_string(),
        seq: 42,
        operation_id: "op_golden_switch".to_string(),
        correlation_id: "corr_golden_switch".to_string(),
        causation_id: Some("evt_golden_safety".to_string()),
        idempotency_key: "idem_golden_switch".to_string(),
        created_at: MessageField::some(sample_timestamp()),
        actor: MessageField::some(Actor {
            r#type: EnumValue::Known(ActorType::Kernel),
            id: "session-kernel".to_string(),
            ..Actor::default()
        }),
        payload: MessageField::some(SessionEventPayload {
            kind: Some(
                ModelSwitchedPayload {
                    from_runner: "openrouter".to_string(),
                    from_model: "anthropic/claude-sonnet".to_string(),
                    to_runner: "xai".to_string(),
                    to_model: "grok-code-fast".to_string(),
                    reason: EnumValue::Known(ModelSwitchReason::UserRequested),
                    ..ModelSwitchedPayload::default()
                }
                .into(),
            ),
            ..SessionEventPayload::default()
        }),
        ..SessionEvent::default()
    }
}

pub fn sample_context_twin() -> ContextTwin {
    ContextTwin {
        schema_version: SCHEMA_VERSION_V1,
        session_id: "sess_golden".to_string(),
        current_objective: "Implement session kernel contracts".to_string(),
        active_plan: "Land protobuf contracts and golden fixtures".to_string(),
        decisions: vec!["Use protobuf as source of truth".to_string()],
        relevant_files: vec!["rsworkspace/proto/trogonai/session/v1/session_event.proto".to_string()],
        user_constraints: vec!["No git commit in this PR".to_string()],
        open_errors: Vec::new(),
        open_risks: Vec::new(),
        test_executions: Vec::new(),
        relevant_tool_results: Vec::new(),
        artifacts: Vec::new(),
        next_steps: vec!["Add session kernel lease policy".to_string()],
        updated_at: MessageField::some(sample_timestamp()),
        derived_from_seq: 41,
        ..ContextTwin::default()
    }
}

pub fn sample_artifact_metadata() -> ArtifactMetadata {
    ArtifactMetadata {
        schema_version: SCHEMA_VERSION_V1,
        artifact_id: "artifact_golden_log".to_string(),
        session_id: "sess_golden".to_string(),
        event_id: "evt_golden_tool_done".to_string(),
        tool_execution_id: Some("texec_golden_bash".to_string()),
        sha256: "abc123".to_string(),
        size_bytes: 4096,
        mime: "text/plain".to_string(),
        preview: "cargo test output...".to_string(),
        storage_ref: "obj://ACP_SESSION_ARTIFACTS/sess_golden/artifact_golden_log".to_string(),
        created_at: MessageField::some(sample_timestamp()),
        retention_policy: EnumValue::Known(ArtifactRetentionPolicy::Session),
        permission_scope: "workspace:default".to_string(),
        encryption_status: EnumValue::Known(EncryptionStatus::None),
        truncated: true,
        ..ArtifactMetadata::default()
    }
}

pub fn sample_capability_schema() -> CapabilitySchema {
    CapabilitySchema {
        schema_version: SCHEMA_VERSION_V1,
        model_id: "grok-code-fast".to_string(),
        runner_id: "xai".to_string(),
        max_context_tokens: 131_072,
        max_output_tokens: 16_384,
        tool_use: true,
        parallel_tool_calls: true,
        image_input: false,
        file_input: true,
        structured_output: true,
        json_schema: true,
        reasoning: false,
        streaming: true,
        tool_result_format: EnumValue::Known(ToolResultFormat::Json),
        system_prompt_support: true,
        provider_restrictions: vec!["no-image-input".to_string()],
        compaction_supported: true,
        source: EnumValue::Known(CapabilitySource::Registry),
        last_verified_at: MessageField::some(sample_timestamp()),
        ttl_seconds: 86_400,
        confidence: 0.95,
        test_results: Vec::new(),
        ..CapabilitySchema::default()
    }
}

pub fn sample_runner_binding() -> RunnerBinding {
    RunnerBinding {
        schema_version: SCHEMA_VERSION_V1,
        session_id: "sess_golden".to_string(),
        runner_id: "xai".to_string(),
        model_id: "grok-code-fast".to_string(),
        status: EnumValue::Known(RunnerBindingStatus::Attached),
        attached_at: MessageField::some(sample_timestamp()),
        detached_at: MessageField::none(),
        detach_reason: None,
        capability_snapshot_id: Some("cap_snap_golden".to_string()),
        ..RunnerBinding::default()
    }
}

pub fn sample_session_snapshot() -> SessionSnapshot {
    SessionSnapshot {
        schema_version: SCHEMA_VERSION_V1,
        session_id: "sess_golden".to_string(),
        last_applied_seq: 42,
        state: MessageField::some(SessionSnapshotState {
            context_twin: MessageField::some(sample_context_twin()),
            active_runner_binding: MessageField::some(sample_runner_binding()),
            ..SessionSnapshotState::default()
        }),
        materialized_at: MessageField::some(sample_timestamp()),
        ..SessionSnapshot::default()
    }
}

/// Minimal v1 event omitting optional nested payload details (N-1 reader input).
pub fn minimal_session_event_wire() -> SessionEvent {
    SessionEvent {
        schema_version: 0,
        event_id: "evt_compat_minimal".to_string(),
        session_id: "sess_compat".to_string(),
        seq: 7,
        operation_id: "op_compat".to_string(),
        correlation_id: "corr_compat".to_string(),
        idempotency_key: "idem_compat".to_string(),
        ..SessionEvent::default()
    }
}

pub fn write_golden_fixtures(dir: &std::path::Path) {
    std::fs::create_dir_all(dir).expect("create fixture dir");

    let fixtures: [(&str, Vec<u8>); 6] = [
        (
            "session_event_session_created_v1.bin",
            sample_session_event().encode_to_vec(),
        ),
        (
            "session_event_model_switched_v1.bin",
            sample_model_switched_event().encode_to_vec(),
        ),
        (
            "context_twin_v1.bin",
            sample_context_twin().encode_to_vec(),
        ),
        (
            "artifact_metadata_v1.bin",
            sample_artifact_metadata().encode_to_vec(),
        ),
        (
            "capability_schema_v1.bin",
            sample_capability_schema().encode_to_vec(),
        ),
        (
            "session_snapshot_v1.bin",
            sample_session_snapshot().encode_to_vec(),
        ),
    ];

    for (name, bytes) in fixtures {
        std::fs::write(dir.join(name), bytes).expect("write golden fixture");
    }
}

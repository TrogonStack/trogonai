use buffa::{EnumValue, MessageField};
use buffa_types::google::protobuf::Timestamp;
use trogonai_capabilities::{FreshnessStatus, ResolvedCapabilities, build_adaptation_plan, detect_session_capability_usage};
use trogonai_session_contracts::{
    ArtifactRef, CapabilityAdaptationAction, CapabilitySchema, CapabilitySource, ContentBlock,
    DegradationKind, ProjectionBlockKind, SCHEMA_VERSION_V1, SessionConfig,
    SessionSnapshotState, SwitchAdaptation, ToolResultFormat,
    __buffa::oneof::content_block::Kind as BlockKind,
};
use trogonai_session_projection::{
    DefaultPromptCompiler, ProjectionConfig, ProjectionInput, PromptCompiler, derive_context_twin,
};

fn timestamp() -> Timestamp {
    Timestamp {
        seconds: 1_748_995_200,
        nanos: 0,
        ..Timestamp::default()
    }
}

fn claude_capabilities() -> ResolvedCapabilities {
    ResolvedCapabilities {
        schema: CapabilitySchema {
            schema_version: SCHEMA_VERSION_V1,
            model_id: "anthropic/claude-sonnet".to_string(),
            runner_id: "openrouter".to_string(),
            max_context_tokens: 200_000,
            tool_use: true,
            image_input: true,
            reasoning: true,
            source: EnumValue::Known(CapabilitySource::Registry),
            tool_result_format: EnumValue::Known(ToolResultFormat::Json),
            ..CapabilitySchema::default()
        },
        runner_id: "openrouter".to_string(),
        freshness: FreshnessStatus::Fresh,
        degraded: false,
    }
}

fn grok_capabilities() -> ResolvedCapabilities {
    ResolvedCapabilities {
        schema: CapabilitySchema {
            schema_version: SCHEMA_VERSION_V1,
            model_id: "xai/grok-code-fast".to_string(),
            runner_id: "xai".to_string(),
            max_context_tokens: 131_072,
            tool_use: true,
            image_input: false,
            reasoning: false,
            source: EnumValue::Known(CapabilitySource::Registry),
            tool_result_format: EnumValue::Known(ToolResultFormat::Json),
            ..CapabilitySchema::default()
        },
        runner_id: "xai".to_string(),
        freshness: FreshnessStatus::Fresh,
        degraded: false,
    }
}

fn fixture_snapshot() -> SessionSnapshotState {
    SessionSnapshotState {
        config: MessageField::some(SessionConfig {
            model: Some("anthropic/claude-sonnet".to_string()),
            runner: Some("openrouter".to_string()),
            compactor_model: Some("xai/grok-code-fast".to_string()),
            system_prompt: Some("You are Trogonai".to_string()),
            permission_rules_text: Some("Never commit unless asked".to_string()),
            ..SessionConfig::default()
        }),
        conversation: vec![
            trogonai_session_contracts::CanonicalMessage {
                message_id: "msg_old".to_string(),
                role: "user".to_string(),
                content: vec![ContentBlock {
                    kind: Some(BlockKind::Text("Start session kernel work".to_string())),
                    ..ContentBlock::default()
                }],
                ..trogonai_session_contracts::CanonicalMessage::default()
            },
            trogonai_session_contracts::CanonicalMessage {
                message_id: "msg_assistant".to_string(),
                role: "assistant".to_string(),
                content: vec![ContentBlock {
                    kind: Some(BlockKind::Text("I'll implement contracts first".to_string())),
                    ..ContentBlock::default()
                }],
                ..trogonai_session_contracts::CanonicalMessage::default()
            },
            trogonai_session_contracts::CanonicalMessage {
                message_id: "msg_current".to_string(),
                role: "user".to_string(),
                content: vec![
                    ContentBlock {
                        kind: Some(BlockKind::Text("Switch to Grok and continue".to_string())),
                        ..ContentBlock::default()
                    },
                    ContentBlock {
                        kind: Some(BlockKind::ImageRef(Box::new(ArtifactRef {
                            artifact_id: "artifact_img".to_string(),
                            preview: "screenshot preview".to_string(),
                            ..ArtifactRef::default()
                        }))),
                        ..ContentBlock::default()
                    },
                ],
                ..trogonai_session_contracts::CanonicalMessage::default()
            },
        ],
        summaries: vec![trogonai_session_contracts::SessionSummary {
            summary_id: "sum_1".to_string(),
            content: "Earlier work established protobuf contracts".to_string(),
            to_seq: 10,
            ..trogonai_session_contracts::SessionSummary::default()
        }],
        ..SessionSnapshotState::default()
    }
}

#[test]
fn deterministic_projection_for_switch_fixture() {
    let snapshot = fixture_snapshot();
    let usage = detect_session_capability_usage(&snapshot);
    let adaptation = build_adaptation_plan(
        "anthropic/claude-sonnet",
        "xai/grok-code-fast",
        &claude_capabilities(),
        &grok_capabilities(),
        &usage,
    );

    let context_twin = derive_context_twin("sess_switch_fixture", &snapshot, 42, timestamp());
    let input = ProjectionInput {
        session_id: "sess_switch_fixture".to_string(),
        model_id: "xai/grok-code-fast".to_string(),
        snapshot,
        context_twin,
        adaptation_plan: Some(adaptation),
        capabilities: grok_capabilities(),
        token_budget: 2_048,
        current_request: None,
        continuity_warnings: vec!["continuity checkpoint recommended".to_string()],
        config: ProjectionConfig {
            output_reserve_tokens: 256,
            recent_turn_count: 2,
            ..ProjectionConfig::default()
        },
        created_at: Some(timestamp()),
        projection_id: Some("proj_switch_fixture".to_string()),
    };

    let compiler = DefaultPromptCompiler;
    let first = compiler.compile(input.clone()).unwrap();
    let second = compiler.compile(input).unwrap();

    assert_eq!(first.projection_id, "proj_switch_fixture");
    assert_eq!(first.session_id, "sess_switch_fixture");
    assert_eq!(first.model_id, "xai/grok-code-fast");
    assert_eq!(first.included_blocks.len(), second.included_blocks.len());
    assert_eq!(first.excluded_blocks.len(), second.excluded_blocks.len());

    let included_kinds: Vec<_> = first
        .included_blocks
        .iter()
        .filter_map(|block| block.kind.as_known())
        .collect();
    assert!(included_kinds.contains(&ProjectionBlockKind::ContextTwin));
    assert!(included_kinds.contains(&ProjectionBlockKind::CurrentRequest));
    assert!(included_kinds.contains(&ProjectionBlockKind::SwitchAdaptation));

    assert!(first.degradation_metadata.iter().any(|entry| {
        entry.kind == EnumValue::Known(DegradationKind::ContextTwinIncluded)
            || entry.kind == EnumValue::Known(DegradationKind::SwitchAdaptationApplied)
    }));

    assert!(first.included_blocks.iter().any(|block| {
        block.block_id == "block_current_request"
            && block.content.iter().any(|content| {
                content
                    .kind
                    .as_ref()
                    .is_some_and(|kind| matches!(kind, BlockKind::Text(text) if text.contains("artifact_img")))
            })
    }));
}

#[test]
fn context_twin_plus_recent_turns_skips_older_transcript() {
    let snapshot = fixture_snapshot();
    let usage = detect_session_capability_usage(&snapshot);
    let mut adaptation = build_adaptation_plan(
        "anthropic/claude-sonnet",
        "xai/grok-code-fast",
        &claude_capabilities(),
        &grok_capabilities(),
        &usage,
    );
    adaptation.adaptations.push(SwitchAdaptation {
        capability: "context_window".to_string(),
        action: EnumValue::Known(CapabilityAdaptationAction::UseContextTwinPlusRecentTurns),
        ..SwitchAdaptation::default()
    });

    let context_twin = derive_context_twin("sess_twin_only", &snapshot, 42, timestamp());
    let input = ProjectionInput {
        session_id: "sess_twin_only".to_string(),
        model_id: "xai/grok-code-fast".to_string(),
        snapshot,
        context_twin,
        adaptation_plan: Some(adaptation),
        capabilities: grok_capabilities(),
        token_budget: 8_192,
        current_request: None,
        continuity_warnings: Vec::new(),
        config: ProjectionConfig::default(),
        created_at: Some(timestamp()),
        projection_id: Some("proj_twin_only".to_string()),
    };

    let projection = DefaultPromptCompiler.compile(input).unwrap();
    assert!(!projection.included_blocks.iter().any(|block| {
        block.kind == EnumValue::Known(ProjectionBlockKind::OlderTranscript)
    }));
    assert!(projection.included_blocks.iter().any(|block| {
        block.kind == EnumValue::Known(ProjectionBlockKind::Summary)
    }));
}

use buffa::{EnumValue, MessageField};
use buffa_types::google::protobuf::Timestamp;
use time::OffsetDateTime;
use trogon_registry::{AgentCapability, MockRegistryStore, Registry};
use trogonai_session_contracts::{
    ArtifactRef, CapabilityAdaptationAction, CapabilitySchema, CapabilitySource,
    ContentBlock, SessionConfig, SessionMetadata, SessionSnapshotState, ToolResultFormat,
    ToolUseBlock, SCHEMA_VERSION_V1, __buffa::oneof::content_block::Kind as BlockKind,
};

use trogonai_capabilities::{
    CapabilityConfig, CapabilityError, CapabilityRegistry, CertificationLevel,
    ProviderCertificationEntry, ProviderCertificationMatrix, ResolvedCapabilities, StaticProbe,
    build_adaptation_plan, create_switch_adaptation_plan, detect_session_capability_usage,
    resolve_model_capabilities, run_probe_battery,
};

fn timestamp(seconds: i64) -> Timestamp {
    Timestamp {
        seconds,
        nanos: 0,
        ..Timestamp::default()
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
        file_input: true,
        structured_output: true,
        json_schema: true,
        reasoning: true,
        streaming: true,
        tool_result_format: EnumValue::Known(ToolResultFormat::Json),
        system_prompt_support: true,
        compaction_supported: true,
        source: EnumValue::Known(CapabilitySource::Registry),
        last_verified_at: MessageField::some(timestamp(1_700_000_000)),
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
        file_input: true,
        structured_output: true,
        json_schema: true,
        reasoning: false,
        streaming: true,
        tool_result_format: EnumValue::Known(ToolResultFormat::Json),
        system_prompt_support: true,
        compaction_supported: true,
        source: EnumValue::Known(CapabilitySource::Registry),
        last_verified_at: MessageField::some(timestamp(1_700_000_000)),
        ttl_seconds: 86_400,
        confidence: 0.95,
        ..CapabilitySchema::default()
    }
}

fn no_tools_schema() -> CapabilitySchema {
    CapabilitySchema {
        schema_version: SCHEMA_VERSION_V1,
        model_id: "text-only".to_string(),
        runner_id: "basic".to_string(),
        max_context_tokens: 8_192,
        max_output_tokens: 1_024,
        tool_use: false,
        image_input: false,
        json_schema: false,
        reasoning: false,
        streaming: true,
        source: EnumValue::Known(CapabilitySource::Registry),
        last_verified_at: MessageField::some(timestamp(1_700_000_000)),
        ttl_seconds: 86_400,
        confidence: 0.9,
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

    let mut basic = AgentCapability::new("basic", ["chat"], "acp.basic.>");
    basic.metadata = serde_json::json!({
        "models": ["text-only"],
    });
    CapabilityRegistry::embed_schema(&mut basic, &no_tools_schema()).unwrap();
    registry.register(&basic).await.unwrap();

    registry
}

fn resolved(schema: CapabilitySchema) -> ResolvedCapabilities {
    ResolvedCapabilities {
        runner_id: schema.runner_id.clone(),
        schema,
        freshness: trogonai_capabilities::FreshnessStatus::Fresh,
        degraded: false,
    }
}

fn session_with_images() -> SessionSnapshotState {
    SessionSnapshotState {
        session: MessageField::some(SessionMetadata {
            id: "sess_test".to_string(),
            ..SessionMetadata::default()
        }),
        config: MessageField::some(SessionConfig {
            model: Some("anthropic/claude-sonnet".to_string()),
            runner: Some("openrouter".to_string()),
            compactor_model: Some("xai/grok-code-fast".to_string()),
            ..SessionConfig::default()
        }),
        conversation: vec![trogonai_session_contracts::CanonicalMessage {
            content: vec![ContentBlock {
                kind: Some(BlockKind::ImageRef(Box::new(ArtifactRef {
                    artifact_id: "artifact_img".to_string(),
                    preview: "screenshot".to_string(),
                    ..ArtifactRef::default()
                }))),
                ..ContentBlock::default()
            }],
            ..trogonai_session_contracts::CanonicalMessage::default()
        }],
        ..SessionSnapshotState::default()
    }
}

#[test]
fn detect_session_usage_flags_images_tools_and_reasoning() {
    let mut session = session_with_images();
    session.conversation.push(trogonai_session_contracts::CanonicalMessage {
        content: vec![
            ContentBlock {
                kind: Some(BlockKind::Thinking("hidden chain".to_string())),
                ..ContentBlock::default()
            },
            ContentBlock {
                kind: Some(BlockKind::ToolUse(Box::new(ToolUseBlock {
                    id: "toolu_1".to_string(),
                    name: "bash".to_string(),
                    input_json: r#"{"cmd":"cargo test"}"#.to_string(),
                    ..ToolUseBlock::default()
                }))),
                ..ContentBlock::default()
            },
        ],
        ..trogonai_session_contracts::CanonicalMessage::default()
    });

    let usage = detect_session_capability_usage(&session);
    assert!(usage.uses_images);
    assert!(usage.uses_tools);
    assert!(usage.uses_reasoning);
}

#[test]
fn build_plan_claude_to_grok_degrades_images_and_reasoning() {
    let usage = detect_session_capability_usage(&session_with_images());
    let plan = build_adaptation_plan(
        "anthropic/claude-sonnet",
        "xai/grok-code-fast",
        &resolved(claude_schema()),
        &resolved(grok_schema()),
        &usage,
    );

    assert_eq!(plan.from_model, "anthropic/claude-sonnet");
    assert_eq!(plan.to_model, "xai/grok-code-fast");
    assert!(plan.adaptations.iter().any(|item| {
        item.capability == "image_input"
            && item.action
                == EnumValue::Known(CapabilityAdaptationAction::UseArtifactRefs)
    }));
    assert!(plan.adaptations.iter().any(|item| {
        item.capability == "reasoning"
            && item.action == EnumValue::Known(CapabilityAdaptationAction::NotPortable)
    }));
    assert!(plan
        .warnings
        .iter()
        .any(|warning| warning.contains("does not support image input")));
}

#[test]
fn build_plan_textualizes_tools_when_target_lacks_tool_use() {
    let mut session = SessionSnapshotState {
        config: MessageField::some(SessionConfig {
            model: Some("anthropic/claude-sonnet".to_string()),
            ..SessionConfig::default()
        }),
        conversation: vec![trogonai_session_contracts::CanonicalMessage {
            content: vec![ContentBlock {
                kind: Some(BlockKind::ToolUse(Box::new(ToolUseBlock {
                    id: "toolu_1".to_string(),
                    name: "bash".to_string(),
                    input_json: "{}".to_string(),
                    ..ToolUseBlock::default()
                }))),
                ..ContentBlock::default()
            }],
            ..trogonai_session_contracts::CanonicalMessage::default()
        }],
        ..SessionSnapshotState::default()
    };
    let usage = detect_session_capability_usage(&session);
    let plan = build_adaptation_plan(
        "anthropic/claude-sonnet",
        "text-only",
        &resolved(claude_schema()),
        &resolved(no_tools_schema()),
        &usage,
    );

    assert!(plan.adaptations.iter().any(|item| {
        item.capability == "tool_use"
            && item.action == EnumValue::Known(CapabilityAdaptationAction::Textualize)
    }));
    assert!(plan.warnings.iter().any(|warning| warning.contains("textualized")));

    session.config = MessageField::none();
    let _ = session;
}

#[tokio::test]
async fn create_switch_adaptation_plan_resolves_registry_capabilities() {
    let registry = registry_with_schemas().await;
    let session = session_with_images();
    let now = OffsetDateTime::from_unix_timestamp(1_700_000_100).unwrap();

    let plan = create_switch_adaptation_plan(
        &registry,
        &session,
        "xai/grok-code-fast",
        now,
        &CapabilityConfig::default(),
    )
    .await
    .expect("plan must be created");

    assert_eq!(plan.to_model, "xai/grok-code-fast");
    assert!(!plan.adaptations.is_empty());
    assert!(plan
        .adaptations
        .iter()
        .any(|item| item.capability == "image_input"));
}

#[tokio::test]
async fn resolve_falls_back_to_kernel_baseline_when_registry_empty() {
    // Certified provider, but nobody registered a schema in AGENT_REGISTRY.
    let registry = Registry::new(MockRegistryStore::new());
    let now = OffsetDateTime::from_unix_timestamp(1_700_000_100).unwrap();

    let resolved = resolve_model_capabilities(
        &registry,
        "anthropic/claude-sonnet-4-5",
        now,
        &CapabilityConfig::default(),
    )
    .await
    .expect("certified model must resolve from the kernel-owned baseline");

    assert_eq!(resolved.runner_id, "claude");
    assert!(resolved.schema.tool_use);
    assert!(resolved.schema.image_input);
    assert!(resolved.schema.reasoning);
    assert!(!resolved.degraded);
}

#[tokio::test]
async fn resolve_errors_for_uncertified_model_when_registry_empty() {
    // Unknown model + empty registry: refuse rather than guess capabilities.
    let registry = Registry::new(MockRegistryStore::new());
    let now = OffsetDateTime::from_unix_timestamp(1_700_000_100).unwrap();

    let err = resolve_model_capabilities(
        &registry,
        "mystery/model-x",
        now,
        &CapabilityConfig::default(),
    )
    .await
    .expect_err("uncharacterized model must not resolve");

    assert!(matches!(err, CapabilityError::ModelNotFound { .. }));
}

#[test]
fn certification_matrix_switch_rules() {
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

    assert!(matrix.is_switch_allowed(
        "anthropic/claude-sonnet",
        "openrouter",
        "xai/grok-code-fast",
        "xai"
    ));
    assert_eq!(
        matrix.certification_level("xai/grok-code-fast", "xai"),
        CertificationLevel::SwitchSafe
    );
}

#[test]
fn probe_battery_reports_expected_capabilities() {
    let probe = StaticProbe {
        tool_use: true,
        image_input: false,
        json_schema: true,
        max_context_tokens: 131_072,
        streaming: true,
        compaction_supported: true,
    };
    let results = run_probe_battery(&probe);
    assert_eq!(results.len(), 6);
    assert!(results.iter().any(|result| result.kind.name() == "tool_use" && result.passed));
    assert!(results.iter().any(|result| result.kind.name() == "image_input" && !result.passed));
}

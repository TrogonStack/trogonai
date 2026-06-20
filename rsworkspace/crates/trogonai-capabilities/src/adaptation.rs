use buffa::{EnumValue, MessageField};
use buffa_types::google::protobuf::Timestamp;
use time::OffsetDateTime;
use trogon_registry::{Registry, RegistryStore};
use trogonai_session_contracts::{
    __buffa::oneof::content_block::Kind as BlockKind, __buffa::oneof::tool_call_result::Kind as ToolResultKind,
    CapabilityAdaptationAction, ContentBlock, SessionSnapshotState, SwitchAdaptation, SwitchAdaptationPlan,
};
use uuid::Uuid;

use crate::config::CapabilityConfig;
use crate::error::CapabilityError;
use crate::resolve::{ResolvedCapabilities, resolve_model_capabilities};
use crate::telemetry::metrics;

/// Capabilities actively used by the canonical session state.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SessionCapabilityUsage {
    pub uses_images: bool,
    pub uses_files: bool,
    pub uses_tools: bool,
    pub uses_reasoning: bool,
    pub uses_json_schema: bool,
    pub uses_large_artifacts: bool,
    pub estimated_context_tokens: u64,
}

/// Detect which capabilities the session has actually used.
pub fn detect_session_capability_usage(session: &SessionSnapshotState) -> SessionCapabilityUsage {
    let mut usage = SessionCapabilityUsage::default();

    for message in &session.conversation {
        for block in &message.content {
            inspect_content_block(block, &mut usage);
        }
        usage.estimated_context_tokens = usage.estimated_context_tokens.saturating_add(
            message
                .usage
                .as_option()
                .map(|u| u.input_tokens + u.output_tokens)
                .unwrap_or(0),
        );
    }

    if !session.tool_calls.is_empty() {
        usage.uses_tools = true;
    }

    for tool in &session.tool_calls {
        if tool
            .result
            .as_option()
            .and_then(|result| result.kind.as_ref())
            .is_some_and(|kind| matches!(kind, ToolResultKind::ArtifactRef(_)))
        {
            usage.uses_large_artifacts = true;
        }
    }

    for artifact in &session.artifacts {
        if artifact.size_bytes > 32 * 1024 {
            usage.uses_large_artifacts = true;
        }
    }

    if session
        .usage
        .as_option()
        .map(|usage| usage.input_tokens + usage.output_tokens)
        .unwrap_or(0)
        > usage.estimated_context_tokens
    {
        usage.estimated_context_tokens = session
            .usage
            .as_option()
            .map(|usage| usage.input_tokens + usage.output_tokens)
            .unwrap_or(0);
    }

    usage
}

fn inspect_content_block(block: &ContentBlock, usage: &mut SessionCapabilityUsage) {
    let Some(kind) = block.kind.as_ref() else {
        return;
    };
    match kind {
        BlockKind::ImageRef(_) => usage.uses_images = true,
        BlockKind::Thinking(_) => usage.uses_reasoning = true,
        BlockKind::ToolUse(_) => usage.uses_tools = true,
        BlockKind::ToolResult(_) => usage.uses_tools = true,
        BlockKind::Text(_) => {}
    }
}

/// Compare current vs target capabilities and build an explicit adaptation plan.
pub fn build_adaptation_plan(
    from_model: &str,
    to_model: &str,
    current: &ResolvedCapabilities,
    target: &ResolvedCapabilities,
    usage: &SessionCapabilityUsage,
) -> SwitchAdaptationPlan {
    let mut adaptations = Vec::new();
    let mut warnings = Vec::new();

    if current.degraded {
        warnings.push(format!(
            "source capabilities for {from_model} were degraded due to stale or unverified registry data"
        ));
    }
    if target.degraded {
        warnings.push(format!(
            "target capabilities for {to_model} were degraded to conservative defaults due to stale or unverified registry data"
        ));
    }

    compare_image_input(current, target, usage, &mut adaptations, &mut warnings);
    compare_tool_use(current, target, usage, &mut adaptations, &mut warnings);
    compare_parallel_tools(current, target, usage, &mut adaptations, &mut warnings);
    compare_context_window(current, target, usage, &mut adaptations, &mut warnings);
    compare_json_schema(current, target, usage, &mut adaptations, &mut warnings);
    compare_reasoning(current, target, usage, &mut adaptations, &mut warnings);
    compare_file_input(current, target, usage, &mut adaptations, &mut warnings);
    compare_large_artifacts(current, target, usage, &mut adaptations, &mut warnings);
    compare_tool_result_format(current, target, usage, &mut adaptations, &mut warnings);
    SwitchAdaptationPlan {
        plan_id: format!("adapt_{}", Uuid::new_v4()),
        from_model: from_model.to_string(),
        to_model: to_model.to_string(),
        adaptations,
        warnings,
        created_at: MessageField::some(now_timestamp()),
        ..SwitchAdaptationPlan::default()
    }
}

/// Create a switch adaptation plan for a session and target model.
pub async fn create_switch_adaptation_plan<S: RegistryStore>(
    registry: &Registry<S>,
    session: &SessionSnapshotState,
    target_model: &str,
    now: OffsetDateTime,
    config: &CapabilityConfig,
) -> Result<SwitchAdaptationPlan, CapabilityError> {
    let from_model = session
        .config
        .as_option()
        .and_then(|config| config.model.clone())
        .ok_or(CapabilityError::SessionModelMissing)?;

    let current = resolve_model_capabilities(registry, &from_model, now, config).await?;
    let target = resolve_model_capabilities(registry, target_model, now, config).await?;
    let usage = detect_session_capability_usage(session);

    let mut plan = build_adaptation_plan(&from_model, target_model, &current, &target, &usage);

    if let Some(session_meta) = session.session.as_option() {
        for adaptation in &plan.adaptations {
            metrics::record_capability_degradation(
                &session_meta.id,
                &adaptation.capability,
                adaptation_action_label(&adaptation.action),
            );
        }
        metrics::record_adaptation_plan_created(&session_meta.id, &from_model, target_model, plan.adaptations.len());
    }

    if let Some(compactor_model) = session
        .config
        .as_option()
        .and_then(|config| config.compactor_model.clone())
        && !target.schema.compaction_supported
    {
        plan.warnings.push(format!(
            "compactor_model {compactor_model} may be unavailable on target model {target_model}; explicit fallback required"
        ));
    }

    Ok(plan)
}

fn compare_image_input(
    current: &ResolvedCapabilities,
    target: &ResolvedCapabilities,
    usage: &SessionCapabilityUsage,
    adaptations: &mut Vec<SwitchAdaptation>,
    warnings: &mut Vec<String>,
) {
    if usage.uses_images {
        if target.schema.image_input {
            push_adaptation(adaptations, "image_input", CapabilityAdaptationAction::Preserve, None);
        } else {
            push_adaptation(
                adaptations,
                "image_input",
                CapabilityAdaptationAction::UseArtifactRefs,
                Some("images converted to artifact references or descriptions".to_string()),
            );
            warnings.push(
                "This model does not support image input; image references were preserved but not sent".to_string(),
            );
        }
    } else if current.schema.image_input && !target.schema.image_input {
        push_adaptation(
            adaptations,
            "image_input",
            CapabilityAdaptationAction::UseArtifactRefs,
            Some("target model lacks image input support".to_string()),
        );
    }
}

fn compare_tool_use(
    current: &ResolvedCapabilities,
    target: &ResolvedCapabilities,
    usage: &SessionCapabilityUsage,
    adaptations: &mut Vec<SwitchAdaptation>,
    warnings: &mut Vec<String>,
) {
    if !usage.uses_tools {
        if target.schema.tool_use {
            push_adaptation(
                adaptations,
                "tool_use",
                CapabilityAdaptationAction::PreserveStructuredTools,
                None,
            );
        }
        return;
    }

    if target.schema.tool_use {
        push_adaptation(
            adaptations,
            "tool_use",
            CapabilityAdaptationAction::PreserveStructuredTools,
            None,
        );
    } else {
        push_adaptation(
            adaptations,
            "tool_use",
            CapabilityAdaptationAction::Textualize,
            Some("past tool calls converted to transcript text".to_string()),
        );
        warnings
            .push("Target model does not support structured tools; past tool calls will be textualized".to_string());
    }

    let _ = current;
}

fn compare_parallel_tools(
    current: &ResolvedCapabilities,
    target: &ResolvedCapabilities,
    usage: &SessionCapabilityUsage,
    adaptations: &mut Vec<SwitchAdaptation>,
    warnings: &mut Vec<String>,
) {
    if !usage.uses_tools {
        return;
    }
    if current.schema.parallel_tool_calls && !target.schema.parallel_tool_calls {
        push_adaptation(
            adaptations,
            "parallel_tool_calls",
            CapabilityAdaptationAction::Textualize,
            Some("parallel tool history summarized sequentially".to_string()),
        );
        warnings.push("Target model does not support parallel tool calls".to_string());
    }
}

fn compare_context_window(
    current: &ResolvedCapabilities,
    target: &ResolvedCapabilities,
    usage: &SessionCapabilityUsage,
    adaptations: &mut Vec<SwitchAdaptation>,
    warnings: &mut Vec<String>,
) {
    let target_window = target.schema.max_context_tokens;
    let needs_projection =
        usage.estimated_context_tokens > target_window || current.schema.max_context_tokens > target_window;

    if needs_projection {
        push_adaptation(
            adaptations,
            "context_window",
            CapabilityAdaptationAction::UseContextTwinPlusRecentTurns,
            Some(format!(
                "history exceeds target context window ({target_window} tokens)"
            )),
        );
        warnings.push("Using Context Twin plus recent turns".to_string());
    } else {
        push_adaptation(
            adaptations,
            "context_window",
            CapabilityAdaptationAction::Preserve,
            None,
        );
    }
}

fn compare_json_schema(
    current: &ResolvedCapabilities,
    target: &ResolvedCapabilities,
    usage: &SessionCapabilityUsage,
    adaptations: &mut Vec<SwitchAdaptation>,
    warnings: &mut Vec<String>,
) {
    if usage.uses_json_schema || current.schema.json_schema {
        if target.schema.json_schema {
            push_adaptation(adaptations, "json_schema", CapabilityAdaptationAction::Preserve, None);
        } else {
            push_adaptation(
                adaptations,
                "json_schema",
                CapabilityAdaptationAction::NotPortable,
                Some("structured JSON schema output is not supported by target model".to_string()),
            );
            warnings.push("JSON schema output is not portable to the target model".to_string());
        }
    }
}

fn compare_reasoning(
    _current: &ResolvedCapabilities,
    target: &ResolvedCapabilities,
    usage: &SessionCapabilityUsage,
    adaptations: &mut Vec<SwitchAdaptation>,
    warnings: &mut Vec<String>,
) {
    if usage.uses_reasoning || _current.schema.reasoning {
        if target.schema.reasoning {
            push_adaptation(adaptations, "reasoning", CapabilityAdaptationAction::Preserve, None);
        } else {
            push_adaptation(
                adaptations,
                "reasoning",
                CapabilityAdaptationAction::NotPortable,
                Some("hidden reasoning blocks are not portable".to_string()),
            );
            warnings.push("Reasoning/thinking content is not portable to the target model".to_string());
        }
    }
}

fn compare_file_input(
    _current: &ResolvedCapabilities,
    target: &ResolvedCapabilities,
    usage: &SessionCapabilityUsage,
    adaptations: &mut Vec<SwitchAdaptation>,
    warnings: &mut Vec<String>,
) {
    if usage.uses_files {
        if target.schema.file_input {
            push_adaptation(adaptations, "file_input", CapabilityAdaptationAction::Preserve, None);
        } else {
            push_adaptation(
                adaptations,
                "file_input",
                CapabilityAdaptationAction::UseArtifactRefs,
                Some("file inputs converted to artifact references".to_string()),
            );
            warnings.push("Target model does not support file input; files remain as artifact refs".to_string());
        }
    }
}

fn compare_large_artifacts(
    _current: &ResolvedCapabilities,
    target: &ResolvedCapabilities,
    usage: &SessionCapabilityUsage,
    adaptations: &mut Vec<SwitchAdaptation>,
    warnings: &mut Vec<String>,
) {
    if usage.uses_large_artifacts {
        push_adaptation(
            adaptations,
            "artifact_refs",
            CapabilityAdaptationAction::UseArtifactRefs,
            Some("large outputs projected as preview plus artifact ref".to_string()),
        );
        if !target.schema.file_input {
            warnings.push("Large tool outputs will use preview plus artifact references".to_string());
        }
    }
}

fn compare_tool_result_format(
    current: &ResolvedCapabilities,
    target: &ResolvedCapabilities,
    usage: &SessionCapabilityUsage,
    adaptations: &mut Vec<SwitchAdaptation>,
    warnings: &mut Vec<String>,
) {
    if !usage.uses_tools {
        return;
    }
    if current.schema.tool_result_format != target.schema.tool_result_format {
        push_adaptation(
            adaptations,
            "tool_result_format",
            CapabilityAdaptationAction::Textualize,
            Some("tool result format differs between source and target models".to_string()),
        );
        warnings.push("Tool result format differs; results may be textualized in projection".to_string());
    }
}

fn push_adaptation(
    adaptations: &mut Vec<SwitchAdaptation>,
    capability: &str,
    action: CapabilityAdaptationAction,
    detail: Option<String>,
) {
    adaptations.push(SwitchAdaptation {
        capability: capability.to_string(),
        action: EnumValue::Known(action),
        detail,
        ..SwitchAdaptation::default()
    });
}

fn adaptation_action_label(action: &EnumValue<CapabilityAdaptationAction>) -> &'static str {
    match action.as_known() {
        Some(CapabilityAdaptationAction::Preserve) => "preserve",
        Some(CapabilityAdaptationAction::UseArtifactRefs) => "use_artifact_refs",
        Some(CapabilityAdaptationAction::PreserveStructuredTools) => "preserve_structured_tools",
        Some(CapabilityAdaptationAction::UseContextTwinPlusRecentTurns) => "use_context_twin_plus_recent_turns",
        Some(CapabilityAdaptationAction::Textualize) => "textualize",
        Some(CapabilityAdaptationAction::Omit) => "omit",
        Some(CapabilityAdaptationAction::NotPortable) => "not_portable",
        _ => "unspecified",
    }
}

fn now_timestamp() -> Timestamp {
    let now = OffsetDateTime::now_utc();
    Timestamp {
        seconds: now.unix_timestamp(),
        nanos: now.nanosecond() as i32,
        ..Timestamp::default()
    }
}

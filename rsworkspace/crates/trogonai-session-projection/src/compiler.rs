use buffa::{EnumValue, MessageField};
use buffa_types::google::protobuf::Timestamp;
use time::OffsetDateTime;
use trogonai_session_contracts::{
    CapabilityAdaptationAction, ContentBlock, DegradationKind, DegradationMetadata, ProjectionBlock,
    ProjectionBlockKind, PromptProjection, SCHEMA_VERSION_V1, SwitchAdaptationPlan, ToolCallResult,
    ToolUseBlock, __buffa::oneof::content_block::Kind as BlockKind,
    __buffa::oneof::tool_call_result::Kind as ToolResultKind,
};

use crate::error::ProjectionError;
use crate::input::ProjectionInput;
use crate::telemetry;
use crate::token_budget::{TokenBudget, block_priority, estimate_content_tokens};

/// Deterministic prompt compiler.
pub trait PromptCompiler {
    fn compile(&self, input: ProjectionInput) -> Result<PromptProjection, ProjectionError>;
}

/// Default deterministic prompt compiler implementing the token-budget priority policy.
#[derive(Clone, Debug, Default)]
pub struct DefaultPromptCompiler;

impl PromptCompiler for DefaultPromptCompiler {
    fn compile(&self, input: ProjectionInput) -> Result<PromptProjection, ProjectionError> {
        compile_projection(input)
    }
}

struct CandidateBlock {
    block: ProjectionBlock,
}

pub(crate) fn compile_projection(input: ProjectionInput) -> Result<PromptProjection, ProjectionError> {
    let budget = TokenBudget::from_model_window(
        input.token_budget_for_model(),
        input.config.output_reserve_tokens,
    );
    let available = budget.available_input_tokens();

    let textualize_tools = adaptation_action(&input.adaptation_plan, "tool_use")
        .is_some_and(|action| matches!(action, CapabilityAdaptationAction::Textualize));
    let use_artifact_refs = adaptation_action(&input.adaptation_plan, "image_input")
        .is_some_and(|action| matches!(action, CapabilityAdaptationAction::UseArtifactRefs));
    let omit_reasoning = adaptation_action(&input.adaptation_plan, "reasoning")
        .is_some_and(|action| matches!(action, CapabilityAdaptationAction::NotPortable));
    let prefer_context_twin = adaptation_action(&input.adaptation_plan, "context_window").is_some_and(
        |action| matches!(action, CapabilityAdaptationAction::UseContextTwinPlusRecentTurns),
    );

    let mut candidates = Vec::new();
    let mut degradations = Vec::new();

    push_system_rules(&input, &mut candidates);
    push_safety_permissions(&input, &mut candidates);
    push_context_twin(&input, &mut candidates, &mut degradations);
    push_switch_adaptation(&input, &mut candidates, &mut degradations);
    push_continuity_warnings(&input, &mut candidates);
    push_current_request(&input, &mut candidates, use_artifact_refs, omit_reasoning, textualize_tools);
    push_recent_turns(
        &input,
        &mut candidates,
        use_artifact_refs,
        omit_reasoning,
        textualize_tools,
        prefer_context_twin,
    );
    push_tool_schemas(&input, &mut candidates);
    push_artifact_previews(&input, &mut candidates, &mut degradations);
    push_unresolved_errors(&input, &mut candidates);
    push_summaries(&input, &mut candidates, prefer_context_twin);
    push_older_transcript(
        &input,
        &mut candidates,
        use_artifact_refs,
        omit_reasoning,
        textualize_tools,
        prefer_context_twin,
    );

    // Component 12: record the content-degradation metadata for the transformations
    // the projection applied (images omitted, tool calls textualized, reasoning not
    // portable, history summarized), so the runner/UX can surface what was lost.
    record_content_degradations(
        &input,
        &mut degradations,
        use_artifact_refs,
        textualize_tools,
        prefer_context_twin,
    );

    candidates.sort_by(|left, right| {
        block_priority(
            left.block
                .kind
                .as_known()
                .unwrap_or(ProjectionBlockKind::Unspecified),
        )
        .cmp(&block_priority(
            right
                .block
                .kind
                .as_known()
                .unwrap_or(ProjectionBlockKind::Unspecified),
        ))
        .then_with(|| left.block.block_id.cmp(&right.block.block_id))
    });

    let mut included = Vec::new();
    let mut excluded = Vec::new();
    let mut used_tokens = 0_u64;

    for candidate in candidates {
        let block_tokens = candidate.block.token_estimate;
        if used_tokens.saturating_add(block_tokens) <= available {
            used_tokens = used_tokens.saturating_add(block_tokens);
            included.push(candidate.block);
            continue;
        }

        degradations.push(DegradationMetadata {
            kind: EnumValue::Known(DegradationKind::BlockExcluded),
            detail: format!(
                "excluded {} due to token budget ({available} available, {used_tokens} used)",
                candidate.block.label
            ),
            block_id: Some(candidate.block.block_id.clone()),
            ..DegradationMetadata::default()
        });
        excluded.push(candidate.block);
    }

    ensure_critical_blocks_fit(&input.model_id, &included)?;

    if !excluded.is_empty() {
        telemetry::metrics::record_projection_degraded(&input.session_id, &input.model_id, excluded.len());
    }

    let projection = PromptProjection {
        schema_version: SCHEMA_VERSION_V1,
        projection_id: input
            .projection_id
            .unwrap_or_else(|| format!("proj_{}", uuid::Uuid::now_v7())),
        session_id: input.session_id.clone(),
        model_id: input.model_id.clone(),
        token_estimate: used_tokens,
        included_blocks: included,
        excluded_blocks: excluded,
        degradation_metadata: degradations,
        capability_snapshot: MessageField::some(input.capabilities.schema.clone()),
        created_at: MessageField::some(input.created_at.unwrap_or_else(now_timestamp)),
        ..PromptProjection::default()
    };

    telemetry::metrics::record_prompt_compiled(
        &projection.session_id,
        &projection.model_id,
        projection.token_estimate,
    );

    Ok(projection)
}

fn ensure_critical_blocks_fit(
    model_id: &str,
    included: &[ProjectionBlock],
) -> Result<(), ProjectionError> {
    let has_system = included.iter().any(|block| {
        block.kind == EnumValue::Known(ProjectionBlockKind::SystemRules)
            || block.kind == EnumValue::Known(ProjectionBlockKind::SafetyPermissions)
    });
    let has_context_twin = included
        .iter()
        .any(|block| block.kind == EnumValue::Known(ProjectionBlockKind::ContextTwin));
    let has_current_request = included
        .iter()
        .any(|block| block.kind == EnumValue::Known(ProjectionBlockKind::CurrentRequest));

    if has_system && has_context_twin && has_current_request {
        return Ok(());
    }

    Err(ProjectionError::CriticalBlocksDoNotFit {
        model_id: model_id.to_string(),
    })
}

fn push_system_rules(input: &ProjectionInput, candidates: &mut Vec<CandidateBlock>) {
    let mut parts = Vec::new();
    if let Some(config) = input.snapshot.config.as_option() {
        if let Some(system_prompt) = config.system_prompt.clone() {
            parts.push(system_prompt);
        }
        if let Some(override_prompt) = config.system_prompt_override.clone() {
            parts.push(override_prompt);
        }
    }
    if parts.is_empty() {
        return;
    }

    let text = parts.join("\n\n");
    let content = vec![text_block(&text)];
    candidates.push(CandidateBlock {
        block: projection_block(
            "block_system_rules",
            ProjectionBlockKind::SystemRules,
            "system/developer/session rules",
            content,
            0,
            None,
        ),
    });
}

fn push_safety_permissions(input: &ProjectionInput, candidates: &mut Vec<CandidateBlock>) {
    let mut parts = Vec::new();
    if let Some(config) = input.snapshot.config.as_option() {
        if let Some(rules) = config.permission_rules_text.clone() {
            parts.push(format!("permission rules:\n{rules}"));
        }
        if !config.tool_policies_json.is_empty() {
            parts.push(format!(
                "tool policies:\n{}",
                config.tool_policies_json.join("\n")
            ));
        }
        if config.disabled_builtin_tools {
            parts.push("builtin tools disabled".to_string());
        }
    }
    if parts.is_empty() {
        return;
    }

    let text = parts.join("\n\n");
    candidates.push(CandidateBlock {
        block: projection_block(
            "block_safety_permissions",
            ProjectionBlockKind::SafetyPermissions,
            "safety, permissions and tool policy",
            vec![text_block(&text)],
            0,
            None,
        ),
    });
}

/// § Prompt Compiler priority order (#8): "active tool schemas necesarias". The active
/// tools are those the session has actually used (the distinct tool names from
/// `tool_calls`); the projection lists them so the target model's token budget accounts
/// for the tool surface and the model knows which tools are in play. The full JSON tool
/// schemas are reloaded by the runner on attach (§ MCP and Tool Lifecycle); the canonical
/// projection view here is the active tool set.
fn push_tool_schemas(input: &ProjectionInput, candidates: &mut Vec<CandidateBlock>) {
    let mut tools: Vec<String> = Vec::new();
    for call in &input.snapshot.tool_calls {
        if !call.name.is_empty() && !tools.iter().any(|name| name == &call.name) {
            tools.push(call.name.clone());
        }
    }
    if tools.is_empty() {
        return;
    }
    let text = format!("active tool schemas: {}", tools.join(", "));
    candidates.push(CandidateBlock {
        block: projection_block(
            "block_tool_schemas",
            ProjectionBlockKind::ToolSchema,
            "active tool schemas",
            vec![text_block(&text)],
            0,
            None,
        ),
    });
}

fn push_context_twin(
    input: &ProjectionInput,
    candidates: &mut Vec<CandidateBlock>,
    degradations: &mut Vec<DegradationMetadata>,
) {
    let twin = &input.context_twin;
    let text = format!(
        "Objective: {}\nPlan: {}\nDecisions: {}\nRelevant files: {}\nConstraints: {}\nOpen errors: {}\nOpen risks: {}\nNext steps: {}",
        twin.current_objective,
        twin.active_plan,
        twin.decisions.join("; "),
        twin.relevant_files.join(", "),
        twin.user_constraints.join("; "),
        twin.open_errors.join("; "),
        twin.open_risks.join("; "),
        twin.next_steps.join("; "),
    );
    candidates.push(CandidateBlock {
        block: projection_block(
            "block_context_twin",
            ProjectionBlockKind::ContextTwin,
            "context twin",
            vec![text_block(&text)],
            0,
            None,
        ),
    });
    degradations.push(DegradationMetadata {
        kind: EnumValue::Known(DegradationKind::ContextTwinIncluded),
        detail: "context twin included in projection".to_string(),
        block_id: Some("block_context_twin".to_string()),
        ..DegradationMetadata::default()
    });
}

fn push_switch_adaptation(
    input: &ProjectionInput,
    candidates: &mut Vec<CandidateBlock>,
    degradations: &mut Vec<DegradationMetadata>,
) {
    let Some(plan) = input.adaptation_plan.as_ref() else {
        return;
    };

    let mut lines = vec![
        format!("switch adaptation plan {}", plan.plan_id),
        format!("from {} to {}", plan.from_model, plan.to_model),
    ];
    for adaptation in &plan.adaptations {
        lines.push(format!(
            "- {} -> {:?}",
            adaptation.capability,
            adaptation.action.as_known()
        ));
        if let Some(detail) = &adaptation.detail {
            lines.push(format!("  {detail}"));
        }
    }
    for warning in &plan.warnings {
        lines.push(format!("warning: {warning}"));
    }

    candidates.push(CandidateBlock {
        block: projection_block(
            "block_switch_adaptation",
            ProjectionBlockKind::SwitchAdaptation,
            "switch adaptation plan",
            vec![text_block(&lines.join("\n"))],
            0,
            None,
        ),
    });
    degradations.push(DegradationMetadata {
        kind: EnumValue::Known(DegradationKind::SwitchAdaptationApplied),
        detail: "switch adaptation plan applied to projection".to_string(),
        block_id: Some("block_switch_adaptation".to_string()),
        ..DegradationMetadata::default()
    });
}

fn push_continuity_warnings(input: &ProjectionInput, candidates: &mut Vec<CandidateBlock>) {
    if input.continuity_warnings.is_empty() {
        return;
    }
    candidates.push(CandidateBlock {
        block: projection_block(
            "block_continuity_warnings",
            ProjectionBlockKind::ContinuityWarning,
            "continuity/force-switch warnings",
            vec![text_block(&input.continuity_warnings.join("\n"))],
            0,
            None,
        ),
    });
}

fn push_current_request(
    input: &ProjectionInput,
    candidates: &mut Vec<CandidateBlock>,
    use_artifact_refs: bool,
    omit_reasoning: bool,
    textualize_tools: bool,
) {
    let Some(message) = input.current_request.as_ref().or_else(|| {
        input
            .snapshot
            .conversation
            .iter()
            .rev()
            .find(|entry| entry.role == "user")
    }) else {
        return;
    };

    let content = project_message(message, use_artifact_refs, omit_reasoning, textualize_tools);
    candidates.push(CandidateBlock {
        block: projection_block(
            "block_current_request",
            ProjectionBlockKind::CurrentRequest,
            "current user request",
            content,
            0,
            Some(message.message_id.clone()),
        ),
    });
}

fn push_recent_turns(
    input: &ProjectionInput,
    candidates: &mut Vec<CandidateBlock>,
    use_artifact_refs: bool,
    omit_reasoning: bool,
    textualize_tools: bool,
    prefer_context_twin: bool,
) {
    if prefer_context_twin {
        return;
    }

    let current_id = input
        .current_request
        .as_ref()
        .map(|message| message.message_id.as_str())
        .or_else(|| {
            input
                .snapshot
                .conversation
                .iter()
                .rev()
                .find(|entry| entry.role == "user")
                .map(|entry| entry.message_id.as_str())
        });

    let mut recent: Vec<_> = input.snapshot.conversation.iter().collect();
    recent.reverse();
    let mut count = 0;
    for message in recent {
        if current_id.is_some_and(|id| id == message.message_id) {
            continue;
        }
        if count >= input.config.recent_turn_count {
            break;
        }
        count += 1;
        let content = project_message(message, use_artifact_refs, omit_reasoning, textualize_tools);
        candidates.push(CandidateBlock {
            block: projection_block(
                &format!("block_recent_{}", message.message_id),
                ProjectionBlockKind::RecentTurn,
                &format!("recent turn {}", message.message_id),
                content,
                0,
                Some(message.message_id.clone()),
            ),
        });
    }
}

fn push_artifact_previews(
    input: &ProjectionInput,
    candidates: &mut Vec<CandidateBlock>,
    degradations: &mut Vec<DegradationMetadata>,
) {
    for artifact in &input.context_twin.artifacts {
        let text = format!(
            "artifact {} ({}, {} bytes): {}",
            artifact.artifact_id, artifact.mime, artifact.size_bytes, artifact.preview
        );
        candidates.push(CandidateBlock {
            block: projection_block(
                &format!("block_artifact_{}", artifact.artifact_id),
                ProjectionBlockKind::ArtifactPreview,
                &format!("artifact preview {}", artifact.artifact_id),
                vec![text_block(&text)],
                0,
                None,
            ),
        });
        degradations.push(DegradationMetadata {
            kind: EnumValue::Known(DegradationKind::ArtifactsReferenced),
            detail: format!("artifact {} referenced via preview", artifact.artifact_id),
            block_id: Some(format!("block_artifact_{}", artifact.artifact_id)),
            ..DegradationMetadata::default()
        });
    }
}

fn push_unresolved_errors(input: &ProjectionInput, candidates: &mut Vec<CandidateBlock>) {
    if input.context_twin.open_errors.is_empty() && input.context_twin.test_executions.is_empty() {
        return;
    }

    let mut lines = input.context_twin.open_errors.clone();
    for test in &input.context_twin.test_executions {
        lines.push(format!("test {} -> {}", test.name, test.result));
    }

    candidates.push(CandidateBlock {
        block: projection_block(
            "block_unresolved_errors",
            ProjectionBlockKind::UnresolvedError,
            "unresolved errors/tests",
            vec![text_block(&lines.join("\n"))],
            0,
            None,
        ),
    });
}

fn push_summaries(
    input: &ProjectionInput,
    candidates: &mut Vec<CandidateBlock>,
    prefer_context_twin: bool,
) {
    if !prefer_context_twin {
        return;
    }

    for summary in &input.snapshot.summaries {
        candidates.push(CandidateBlock {
            block: projection_block(
                &format!("block_summary_{}", summary.summary_id),
                ProjectionBlockKind::Summary,
                &format!("summary {}", summary.summary_id),
                vec![text_block(&summary.content)],
                summary.to_seq,
                None,
            ),
        });
    }
}

fn push_older_transcript(
    input: &ProjectionInput,
    candidates: &mut Vec<CandidateBlock>,
    use_artifact_refs: bool,
    omit_reasoning: bool,
    textualize_tools: bool,
    prefer_context_twin: bool,
) {
    if prefer_context_twin {
        return;
    }

    let recent_ids: std::collections::HashSet<_> = input
        .snapshot
        .conversation
        .iter()
        .rev()
        .take(input.config.recent_turn_count + 1)
        .map(|message| message.message_id.as_str())
        .collect();

    for message in &input.snapshot.conversation {
        if recent_ids.contains(message.message_id.as_str()) {
            continue;
        }
        let content = project_message(message, use_artifact_refs, omit_reasoning, textualize_tools);
        candidates.push(CandidateBlock {
            block: projection_block(
                &format!("block_older_{}", message.message_id),
                ProjectionBlockKind::OlderTranscript,
                &format!("older transcript {}", message.message_id),
                content,
                0,
                Some(message.message_id.clone()),
            ),
        });
    }
}

/// True when any message in the snapshot carries a content block matching `pred`.
fn conversation_has_block(
    snapshot: &trogonai_session_contracts::SessionSnapshotState,
    pred: impl Fn(&BlockKind) -> bool,
) -> bool {
    snapshot
        .conversation
        .iter()
        .any(|message| message.content.iter().any(|block| block.kind.as_ref().is_some_and(&pred)))
}

/// Record the degradation metadata for the content transformations the projection
/// applied (Component 12: `images_omitted`, `tool_calls_textualized`,
/// `reasoning_not_portable`, `history_summarized`). Each is emitted only when the
/// projection actually degrades that content: the adaptation strategy is active and
/// the canonical session contains the affected block type.
fn record_content_degradations(
    input: &ProjectionInput,
    degradations: &mut Vec<DegradationMetadata>,
    use_artifact_refs: bool,
    textualize_tools: bool,
    prefer_context_twin: bool,
) {
    let snapshot = &input.snapshot;
    let mut record = |kind: DegradationKind, detail: &str| {
        degradations.push(DegradationMetadata {
            kind: EnumValue::Known(kind),
            detail: detail.to_string(),
            ..DegradationMetadata::default()
        });
    };

    if use_artifact_refs && conversation_has_block(snapshot, |k| matches!(k, BlockKind::ImageRef(_))) {
        record(
            DegradationKind::ImagesOmitted,
            "images replaced with artifact references for a model without image input",
        );
    }
    if textualize_tools
        && conversation_has_block(snapshot, |k| {
            matches!(k, BlockKind::ToolUse(_) | BlockKind::ToolResult(_))
        })
    {
        record(
            DegradationKind::ToolCallsTextualized,
            "structured tool calls rendered as text for a model without tool use",
        );
    }
    // Provider hidden reasoning is never portable across a switch (a non-goal), so
    // any reasoning content is degraded in the projection regardless of the target.
    if conversation_has_block(snapshot, |k| matches!(k, BlockKind::Thinking(_))) {
        record(
            DegradationKind::ReasoningNotPortable,
            "provider reasoning is not portable across a model switch",
        );
    }
    // With the context-window adaptation, older transcript is represented by
    // summaries plus recent turns rather than the full history.
    if prefer_context_twin && !snapshot.summaries.is_empty() {
        record(
            DegradationKind::HistorySummarized,
            "older transcript represented by summaries plus recent turns",
        );
    }
}

fn project_message(
    message: &trogonai_session_contracts::CanonicalMessage,
    use_artifact_refs: bool,
    omit_reasoning: bool,
    textualize_tools: bool,
) -> Vec<ContentBlock> {
    let mut projected = Vec::new();
    for block in &message.content {
        match block.kind.as_ref() {
            Some(BlockKind::Text(text)) => projected.push(text_block(text)),
            Some(BlockKind::ImageRef(artifact)) if use_artifact_refs => {
                projected.push(text_block(&format!(
                    "[image omitted; artifact {} preview: {}]",
                    artifact.artifact_id, artifact.preview
                )));
            }
            Some(BlockKind::ImageRef(_)) => {}
            Some(BlockKind::Thinking(_)) if omit_reasoning => {}
            Some(BlockKind::Thinking(thinking)) => projected.push(ContentBlock {
                kind: Some(BlockKind::Text(format!("[reasoning omitted: {thinking}]"))),
                ..ContentBlock::default()
            }),
            Some(BlockKind::ToolUse(tool_use)) if textualize_tools => {
                projected.push(text_block(&format!(
                    "[tool {} {} input {}]",
                    tool_use.name, tool_use.id, tool_use.input_json
                )));
            }
            Some(BlockKind::ToolUse(tool_use)) => projected.push(ContentBlock {
                kind: Some(BlockKind::ToolUse(Box::new(ToolUseBlock {
                    id: tool_use.id.clone(),
                    name: tool_use.name.clone(),
                    input_json: tool_use.input_json.clone(),
                    parent_tool_use_id: tool_use.parent_tool_use_id.clone(),
                    ..ToolUseBlock::default()
                }))),
                ..ContentBlock::default()
            }),
            Some(BlockKind::ToolResult(tool_result)) if textualize_tools => {
                let rendered = render_tool_result(&tool_result.result);
                projected.push(text_block(&format!(
                    "[tool result for {}: {rendered}]",
                    tool_result.tool_use_id
                )));
            }
            Some(BlockKind::ToolResult(_tool_result)) => projected.push(block.clone()),
            None => {}
        }
    }
    projected
}

fn render_tool_result(result: &MessageField<ToolCallResult>) -> String {
    result
        .as_option()
        .and_then(|result| match result.kind.as_ref() {
            Some(ToolResultKind::Text(text)) => Some(text.content.clone()),
            Some(ToolResultKind::ArtifactRef(artifact)) => Some(format!(
                "artifact {} preview {}",
                artifact.artifact_id, artifact.preview
            )),
            None => None,
        })
        .unwrap_or_else(|| "<empty>".to_string())
}

fn projection_block(
    block_id: &str,
    kind: ProjectionBlockKind,
    label: &str,
    content: Vec<ContentBlock>,
    source_seq: u64,
    source_message_id: Option<String>,
) -> ProjectionBlock {
    let token_estimate = estimate_content_tokens(&content);
    ProjectionBlock {
        block_id: block_id.to_string(),
        kind: EnumValue::Known(kind),
        label: label.to_string(),
        content,
        token_estimate,
        source_seq,
        source_message_id,
        ..ProjectionBlock::default()
    }
}

fn text_block(text: &str) -> ContentBlock {
    ContentBlock {
        kind: Some(BlockKind::Text(text.to_string())),
        ..ContentBlock::default()
    }
}

fn adaptation_action(
    plan: &Option<SwitchAdaptationPlan>,
    capability: &str,
) -> Option<CapabilityAdaptationAction> {
    plan.as_ref()?.adaptations.iter().find_map(|adaptation| {
        (adaptation.capability == capability)
            .then(|| adaptation.action.as_known())
            .flatten()
    })
}

fn now_timestamp() -> Timestamp {
    let now = OffsetDateTime::now_utc();
    Timestamp {
        seconds: now.unix_timestamp(),
        nanos: now.nanosecond() as i32,
        ..Timestamp::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use buffa_types::google::protobuf::Timestamp;
    use trogonai_capabilities::{FreshnessStatus, ResolvedCapabilities};
    use trogonai_session_contracts::{
        ArtifactRef, CanonicalMessage, CapabilitySchema, CapabilitySource, ContextTwin,
        SessionConfig, SessionSnapshotState, SwitchAdaptation, SwitchAdaptationPlan,
        ToolResultFormat, ToolUseBlock,
    };

    fn fixed_timestamp() -> Timestamp {
        Timestamp {
            seconds: 1_748_995_200,
            nanos: 0,
            ..Timestamp::default()
        }
    }

    fn capabilities(max_context: u64) -> ResolvedCapabilities {
        ResolvedCapabilities {
            schema: CapabilitySchema {
                schema_version: SCHEMA_VERSION_V1,
                model_id: "text-only".to_string(),
                runner_id: "basic".to_string(),
                max_context_tokens: max_context,
                tool_use: false,
                image_input: false,
                json_schema: false,
                reasoning: false,
                source: EnumValue::Known(CapabilitySource::Registry),
                tool_result_format: EnumValue::Known(ToolResultFormat::Json),
                ..CapabilitySchema::default()
            },
            runner_id: "basic".to_string(),
            freshness: FreshnessStatus::Fresh,
            degraded: false,
        }
    }

    fn image_block() -> ContentBlock {
        ContentBlock {
            kind: Some(BlockKind::ImageRef(Box::new(ArtifactRef {
                artifact_id: "art_img".to_string(),
                preview: "a diagram".to_string(),
                ..ArtifactRef::default()
            }))),
            ..ContentBlock::default()
        }
    }

    fn tool_use_block() -> ContentBlock {
        ContentBlock {
            kind: Some(BlockKind::ToolUse(Box::new(ToolUseBlock {
                id: "tu_1".to_string(),
                name: "fs_read".to_string(),
                input_json: "{}".to_string(),
                ..ToolUseBlock::default()
            }))),
            ..ContentBlock::default()
        }
    }

    fn thinking_block() -> ContentBlock {
        ContentBlock {
            kind: Some(BlockKind::Thinking("internal reasoning".to_string())),
            ..ContentBlock::default()
        }
    }

    #[test]
    fn records_content_degradation_for_images_tools_and_reasoning() {
        let input = ProjectionInput {
            session_id: "sess_degrade".to_string(),
            model_id: "text-only".to_string(),
            snapshot: SessionSnapshotState {
                config: MessageField::some(SessionConfig {
                    system_prompt: Some("You are helpful".to_string()),
                    ..SessionConfig::default()
                }),
                conversation: vec![CanonicalMessage {
                    message_id: "msg_mixed".to_string(),
                    role: "user".to_string(),
                    content: vec![
                        text_block("look at this"),
                        image_block(),
                        tool_use_block(),
                        thinking_block(),
                    ],
                    ..CanonicalMessage::default()
                }],
                ..SessionSnapshotState::default()
            },
            context_twin: ContextTwin {
                schema_version: SCHEMA_VERSION_V1,
                session_id: "sess_degrade".to_string(),
                current_objective: "mixed content".to_string(),
                ..ContextTwin::default()
            },
            // Target lacks tools and images -> textualize tools, use artifact refs.
            adaptation_plan: Some(SwitchAdaptationPlan {
                plan_id: "adapt_degrade".to_string(),
                from_model: "anthropic/claude-sonnet".to_string(),
                to_model: "text-only".to_string(),
                adaptations: vec![
                    SwitchAdaptation {
                        capability: "tool_use".to_string(),
                        action: EnumValue::Known(CapabilityAdaptationAction::Textualize),
                        ..SwitchAdaptation::default()
                    },
                    SwitchAdaptation {
                        capability: "image_input".to_string(),
                        action: EnumValue::Known(CapabilityAdaptationAction::UseArtifactRefs),
                        ..SwitchAdaptation::default()
                    },
                ],
                ..SwitchAdaptationPlan::default()
            }),
            capabilities: capabilities(200_000),
            token_budget: 200_000,
            current_request: None,
            continuity_warnings: Vec::new(),
            config: crate::config::ProjectionConfig {
                output_reserve_tokens: 0,
                ..crate::config::ProjectionConfig::default()
            },
            created_at: Some(fixed_timestamp()),
            projection_id: Some("proj_degrade".to_string()),
        };

        let projection = DefaultPromptCompiler.compile(input).unwrap();
        let emitted = |kind: DegradationKind| {
            projection
                .degradation_metadata
                .iter()
                .any(|entry| entry.kind == EnumValue::Known(kind))
        };
        assert!(emitted(DegradationKind::ImagesOmitted), "images_omitted must be recorded");
        assert!(
            emitted(DegradationKind::ToolCallsTextualized),
            "tool_calls_textualized must be recorded"
        );
        assert!(
            emitted(DegradationKind::ReasoningNotPortable),
            "reasoning_not_portable must be recorded"
        );
    }

    #[test]
    fn tool_schemas_block_lists_distinct_active_tools() {
        // § Prompt Compiler priority order #8: the active tool schemas are projected from
        // the distinct tool names the session has used (deduped).
        let input = ProjectionInput {
            session_id: "sess_tools".to_string(),
            model_id: "text-only".to_string(),
            snapshot: SessionSnapshotState {
                config: MessageField::some(SessionConfig {
                    system_prompt: Some("You are helpful".to_string()),
                    ..SessionConfig::default()
                }),
                conversation: vec![trogonai_session_contracts::CanonicalMessage {
                    message_id: "msg_user".to_string(),
                    role: "user".to_string(),
                    content: vec![text_block("run the build")],
                    ..trogonai_session_contracts::CanonicalMessage::default()
                }],
                tool_calls: vec![
                    trogonai_session_contracts::CanonicalToolCall {
                        id: "t1".to_string(),
                        name: "bash".to_string(),
                        ..trogonai_session_contracts::CanonicalToolCall::default()
                    },
                    trogonai_session_contracts::CanonicalToolCall {
                        id: "t2".to_string(),
                        name: "read".to_string(),
                        ..trogonai_session_contracts::CanonicalToolCall::default()
                    },
                    trogonai_session_contracts::CanonicalToolCall {
                        id: "t3".to_string(),
                        name: "bash".to_string(),
                        ..trogonai_session_contracts::CanonicalToolCall::default()
                    },
                ],
                ..SessionSnapshotState::default()
            },
            context_twin: ContextTwin {
                schema_version: SCHEMA_VERSION_V1,
                session_id: "sess_tools".to_string(),
                current_objective: "use tools".to_string(),
                ..ContextTwin::default()
            },
            adaptation_plan: None,
            capabilities: capabilities(200_000),
            token_budget: 200_000,
            current_request: None,
            continuity_warnings: Vec::new(),
            config: crate::config::ProjectionConfig::default(),
            created_at: Some(fixed_timestamp()),
            projection_id: Some("proj_tools".to_string()),
        };

        let projection = DefaultPromptCompiler.compile(input).unwrap();
        let tool_block = projection
            .included_blocks
            .iter()
            .find(|block| block.kind == EnumValue::Known(ProjectionBlockKind::ToolSchema))
            .expect("a ToolSchema block must be produced for the active tools");
        let text: String = tool_block
            .content
            .iter()
            .filter_map(|block| match block.kind.as_ref()? {
                BlockKind::Text(text) => Some(text.clone()),
                _ => None,
            })
            .collect();
        assert!(text.contains("bash") && text.contains("read"));
        assert_eq!(text.matches("bash").count(), 1, "duplicate tool names are deduped");
    }

    #[test]
    fn compile_is_deterministic_for_fixed_input() {
        let input = ProjectionInput {
            session_id: "sess_det".to_string(),
            model_id: "text-only".to_string(),
            snapshot: SessionSnapshotState {
                config: MessageField::some(SessionConfig {
                    system_prompt: Some("You are helpful".to_string()),
                    permission_rules_text: Some("deny destructive tools".to_string()),
                    ..SessionConfig::default()
                }),
                conversation: vec![trogonai_session_contracts::CanonicalMessage {
                    message_id: "msg_user".to_string(),
                    role: "user".to_string(),
                    content: vec![text_block("Implement projection")],
                    ..trogonai_session_contracts::CanonicalMessage::default()
                }],
                ..SessionSnapshotState::default()
            },
            context_twin: ContextTwin {
                schema_version: SCHEMA_VERSION_V1,
                session_id: "sess_det".to_string(),
                current_objective: "Implement projection".to_string(),
                active_plan: "Land crate".to_string(),
                ..ContextTwin::default()
            },
            adaptation_plan: Some(SwitchAdaptationPlan {
                plan_id: "adapt_1".to_string(),
                from_model: "a".to_string(),
                to_model: "text-only".to_string(),
                adaptations: vec![SwitchAdaptation {
                    capability: "tool_use".to_string(),
                    action: EnumValue::Known(CapabilityAdaptationAction::Textualize),
                    ..SwitchAdaptation::default()
                }],
                ..SwitchAdaptationPlan::default()
            }),
            capabilities: capabilities(512),
            token_budget: 4_096,
            current_request: None,
            continuity_warnings: vec!["checkpoint pending".to_string()],
            config: crate::config::ProjectionConfig {
                output_reserve_tokens: 0,
                ..crate::config::ProjectionConfig::default()
            },
            created_at: Some(fixed_timestamp()),
            projection_id: Some("proj_fixed".to_string()),
        };

        let compiler = DefaultPromptCompiler;
        let first = compiler.compile(input.clone()).unwrap();
        let second = compiler.compile(input).unwrap();
        assert_eq!(first.projection_id, "proj_fixed");
        assert_eq!(first.included_blocks.len(), second.included_blocks.len());
        assert_eq!(first.excluded_blocks.len(), second.excluded_blocks.len());
        assert_eq!(
            first
                .included_blocks
                .iter()
                .map(|block| block.block_id.clone())
                .collect::<Vec<_>>(),
            second
                .included_blocks
                .iter()
                .map(|block| block.block_id.clone())
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn low_budget_excludes_lower_priority_blocks() {
        let input = ProjectionInput {
            session_id: "sess_budget".to_string(),
            model_id: "text-only".to_string(),
            snapshot: SessionSnapshotState {
                config: MessageField::some(SessionConfig {
                    system_prompt: Some("rules".to_string()),
                    ..SessionConfig::default()
                }),
                conversation: (0..6)
                    .map(|idx| trogonai_session_contracts::CanonicalMessage {
                        message_id: format!("msg_{idx}"),
                        role: if idx % 2 == 0 { "user" } else { "assistant" }.to_string(),
                        content: vec![text_block(&"word ".repeat(40))],
                        ..trogonai_session_contracts::CanonicalMessage::default()
                    })
                    .collect(),
                summaries: vec![trogonai_session_contracts::SessionSummary {
                    summary_id: "sum_1".to_string(),
                    content: "older history summary".to_string(),
                    to_seq: 1,
                    ..trogonai_session_contracts::SessionSummary::default()
                }],
                ..SessionSnapshotState::default()
            },
            context_twin: ContextTwin {
                schema_version: SCHEMA_VERSION_V1,
                session_id: "sess_budget".to_string(),
                current_objective: "work".to_string(),
                artifacts: vec![ArtifactRef {
                    artifact_id: "artifact_1".to_string(),
                    preview: "preview".to_string(),
                    ..ArtifactRef::default()
                }],
                ..ContextTwin::default()
            },
            adaptation_plan: None,
            capabilities: capabilities(120),
            token_budget: 120,
            current_request: None,
            continuity_warnings: Vec::new(),
            config: crate::config::ProjectionConfig {
                output_reserve_tokens: 0,
                recent_turn_count: 2,
                ..crate::config::ProjectionConfig::default()
            },
            created_at: Some(fixed_timestamp()),
            projection_id: Some("proj_budget".to_string()),
        };

        let projection = DefaultPromptCompiler.compile(input).unwrap();
        assert!(projection.excluded_blocks.iter().any(|block| {
            block.kind == EnumValue::Known(ProjectionBlockKind::OlderTranscript)
                || block.kind == EnumValue::Known(ProjectionBlockKind::ArtifactPreview)
        }));
        assert!(projection
            .degradation_metadata
            .iter()
            .any(|entry| entry.kind == EnumValue::Known(DegradationKind::BlockExcluded)));
    }
}

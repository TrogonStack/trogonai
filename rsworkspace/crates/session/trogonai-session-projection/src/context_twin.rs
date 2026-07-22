use buffa::{EnumValue, MessageField};
use buffa_types::google::protobuf::Timestamp;
use trogonai_session_contracts::{
    ArtifactRef, CanonicalMessage, ContextTwin, RelevantToolResult, SCHEMA_VERSION_V1,
    SessionSnapshotState, TestExecution, ToolCallStatus, __buffa::oneof::content_block::Kind as BlockKind,
    __buffa::oneof::tool_call_result::Kind as ToolResultKind,
};

/// Derive a Context Twin from materialized session state.
///
/// The canonical transcript/event log remains the source of truth; this is a compact operational view.
pub fn derive_context_twin(
    session_id: &str,
    state: &SessionSnapshotState,
    derived_from_seq: u64,
    updated_at: Timestamp,
) -> ContextTwin {
    let current_objective = derive_current_objective(state);
    let active_plan = derive_active_plan(state);
    let decisions = derive_decisions(state);
    let relevant_files = derive_relevant_files(state);
    let user_constraints = derive_user_constraints(state);
    let open_errors = derive_open_errors(state);
    let open_risks = derive_open_risks(state);
    let test_executions = derive_test_executions(state);
    let relevant_tool_results = derive_relevant_tool_results(state);
    let artifacts = derive_artifact_refs(state);
    let next_steps = derive_next_steps(state);

    ContextTwin {
        schema_version: SCHEMA_VERSION_V1,
        session_id: session_id.to_string(),
        current_objective,
        active_plan,
        decisions,
        relevant_files,
        user_constraints,
        open_errors,
        open_risks,
        test_executions,
        relevant_tool_results,
        artifacts,
        next_steps,
        updated_at: MessageField::some(updated_at),
        derived_from_seq,
        ..ContextTwin::default()
    }
}

fn derive_current_objective(state: &SessionSnapshotState) -> String {
    if let Some(last_user) = state
        .conversation
        .iter()
        .rev()
        .find(|message| message.role == "user")
    {
        return summarize_message(last_user);
    }

    state
        .session
        .as_option()
        .map(|session| session.title.clone())
        .filter(|title| !title.is_empty())
        .unwrap_or_else(|| "Continue the active session work".to_string())
}

fn derive_active_plan(state: &SessionSnapshotState) -> String {
    let pending_todos: Vec<_> = state
        .todos
        .iter()
        .filter(|todo| todo.status != "completed" && todo.status != "cancelled")
        .map(|todo| todo.content.clone())
        .collect();
    if !pending_todos.is_empty() {
        return pending_todos.join("; ");
    }

    if let Some(last_assistant) = state
        .conversation
        .iter()
        .rev()
        .find(|message| message.role == "assistant")
    {
        return summarize_message(last_assistant);
    }

    "No explicit plan recorded".to_string()
}

fn derive_decisions(state: &SessionSnapshotState) -> Vec<String> {
    state
        .audit_log
        .iter()
        .filter(|entry| entry.outcome == "approved" || entry.outcome == "decided")
        .map(|entry| format!("{}: {}", entry.tool, entry.input_summary))
        .collect()
}

fn derive_relevant_files(state: &SessionSnapshotState) -> Vec<String> {
    let mut files = Vec::new();
    for tool in &state.tool_calls {
        if (tool.name == "read" || tool.name == "write" || tool.name == "edit" || tool.name == "str_replace")
            && let Some(path) = extract_json_string_field(&tool.input_json, "path")
        {
            push_unique(&mut files, path);
        }
    }
    files
}

fn derive_user_constraints(state: &SessionSnapshotState) -> Vec<String> {
    let mut constraints = Vec::new();
    if let Some(rules) = state
        .config
        .as_option()
        .and_then(|config| config.permission_rules_text.clone())
        && !rules.trim().is_empty()
    {
        constraints.push(rules.trim().to_string());
    }
    if let Some(system_prompt) = state
        .config
        .as_option()
        .and_then(|config| config.system_prompt_override.clone().or_else(|| config.system_prompt.clone()))
        && !system_prompt.trim().is_empty()
    {
        push_unique(&mut constraints, "system prompt override active".to_string());
    }
    constraints
}

fn derive_open_errors(state: &SessionSnapshotState) -> Vec<String> {
    state
        .tool_calls
        .iter()
        .filter(|tool| tool.status == EnumValue::Known(ToolCallStatus::Failed))
        .filter_map(|tool| tool.error.clone())
        .collect()
}

fn derive_open_risks(state: &SessionSnapshotState) -> Vec<String> {
    let mut risks = Vec::new();
    if let Some(safety) = state.switch_safety.as_option() {
        for reason in &safety.reasons {
            risks.push(format!("{}: {}", reason.kind, reason.detail));
        }
    }
    if let Some(plan) = state.switch_adaptation_plan.as_option() {
        risks.extend(plan.warnings.clone());
    }
    risks
}

fn derive_test_executions(state: &SessionSnapshotState) -> Vec<TestExecution> {
    state
        .tool_calls
        .iter()
        .filter(|tool| {
            tool.name.contains("test")
                || tool.input_json.contains("cargo test")
                || tool.input_json.contains("npm test")
        })
        .map(|tool| {
            let result = match tool.status.as_known() {
                Some(ToolCallStatus::Completed) => "passed".to_string(),
                Some(ToolCallStatus::Failed) => "failed".to_string(),
                Some(ToolCallStatus::Started) | Some(ToolCallStatus::Pending) => "running".to_string(),
                _ => "unknown".to_string(),
            };
            TestExecution {
                name: tool.name.clone(),
                result,
                detail: tool.error.clone(),
                ..TestExecution::default()
            }
        })
        .collect()
}

fn derive_relevant_tool_results(state: &SessionSnapshotState) -> Vec<RelevantToolResult> {
    state
        .tool_calls
        .iter()
        .filter(|tool| tool.status == EnumValue::Known(ToolCallStatus::Completed))
        .map(|tool| {
            let summary = tool
                .result
                .as_option()
                .and_then(|result| match result.kind.as_ref() {
                    Some(ToolResultKind::Text(text)) => Some(text.content.clone()),
                    Some(ToolResultKind::ArtifactRef(artifact)) => Some(artifact.preview.clone()),
                    None => None,
                })
                .unwrap_or_default();
            RelevantToolResult {
                tool_call_id: tool.id.clone(),
                tool_name: tool.name.clone(),
                summary: truncate_preview(&summary, 256),
                artifact_ref: match tool.result.as_option().and_then(|result| result.kind.as_ref()) {
                    Some(ToolResultKind::ArtifactRef(artifact)) => MessageField::some(*artifact.clone()),
                    _ => MessageField::none(),
                },
                ..RelevantToolResult::default()
            }
        })
        .collect()
}

fn derive_artifact_refs(state: &SessionSnapshotState) -> Vec<ArtifactRef> {
    state
        .artifacts
        .iter()
        .map(|artifact| ArtifactRef {
            artifact_id: artifact.artifact_id.clone(),
            sha256: artifact.sha256.clone(),
            size_bytes: artifact.size_bytes,
            mime: artifact.mime.clone(),
            preview: artifact.preview.clone(),
            truncated: artifact.truncated,
            ..ArtifactRef::default()
        })
        .collect()
}

fn derive_next_steps(state: &SessionSnapshotState) -> Vec<String> {
    let mut steps = state
        .todos
        .iter()
        .filter(|todo| todo.status == "pending" || todo.status == "in_progress")
        .map(|todo| todo.content.clone())
        .collect::<Vec<_>>();

    if steps.is_empty()
        && let Some(last_assistant) = state
            .conversation
            .iter()
            .rev()
            .find(|message| message.role == "assistant")
    {
        steps.push(summarize_message(last_assistant));
    }

    steps
}

fn summarize_message(message: &CanonicalMessage) -> String {
    let mut parts = Vec::new();
    for block in &message.content {
        if let Some(BlockKind::Text(text)) = block.kind.as_ref() {
            parts.push(text.as_str());
        }
    }
    truncate_preview(&parts.join("\n"), 512)
}

fn truncate_preview(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    text.chars().take(max_chars).collect::<String>() + "..."
}

fn push_unique(values: &mut Vec<String>, value: String) {
    if !values.iter().any(|existing| existing == &value) {
        values.push(value);
    }
}

fn extract_json_string_field(input_json: &str, field: &str) -> Option<String> {
    let pattern = format!("\"{field}\":\"");
    let start = input_json.find(&pattern)? + pattern.len();
    let rest = &input_json[start..];
    let end = rest.find('"')?;
    Some(rest[..end].to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use buffa_types::google::protobuf::Timestamp;
    use trogonai_session_contracts::{
        CanonicalToolCall, ContentBlock, SessionConfig, SessionMetadata, TodoItem,
    };

    fn timestamp() -> Timestamp {
        Timestamp {
            seconds: 1_748_995_200,
            nanos: 0,
            ..Timestamp::default()
        }
    }

    #[test]
    fn derive_context_twin_from_snapshot_state() {
        let state = SessionSnapshotState {
            session: MessageField::some(SessionMetadata {
                id: "sess_proj".to_string(),
                title: "Session kernel".to_string(),
                ..SessionMetadata::default()
            }),
            config: MessageField::some(SessionConfig {
                permission_rules_text: Some("no commits".to_string()),
                ..SessionConfig::default()
            }),
            conversation: vec![CanonicalMessage {
                message_id: "msg_1".to_string(),
                role: "user".to_string(),
                content: vec![ContentBlock {
                    kind: Some(BlockKind::Text("Implement projection crate".to_string())),
                    ..ContentBlock::default()
                }],
                ..CanonicalMessage::default()
            }],
            tool_calls: vec![CanonicalToolCall {
                id: "tool_1".to_string(),
                name: "cargo test".to_string(),
                input_json: r#"{"cmd":"cargo test -p trogonai-session-projection"}"#.to_string(),
                status: EnumValue::Known(ToolCallStatus::Failed),
                error: Some("test failed".to_string()),
                ..CanonicalToolCall::default()
            }],
            todos: vec![TodoItem {
                id: "todo_1".to_string(),
                content: "Add deterministic compiler tests".to_string(),
                status: "pending".to_string(),
                ..TodoItem::default()
            }],
            ..SessionSnapshotState::default()
        };

        let twin = derive_context_twin("sess_proj", &state, 10, timestamp());
        assert_eq!(twin.session_id, "sess_proj");
        assert_eq!(twin.current_objective, "Implement projection crate");
        assert_eq!(twin.derived_from_seq, 10);
        assert_eq!(twin.open_errors, vec!["test failed".to_string()]);
        assert_eq!(twin.next_steps, vec!["Add deterministic compiler tests".to_string()]);
        assert!(twin.user_constraints.iter().any(|c| c.contains("no commits")));
    }
}

use std::collections::{HashMap, HashSet};

use acp_nats::prompt_event::PromptEvent;
use agent_client_protocol::{
    ConfigOptionUpdate, ContentBlock, ContentChunk, CurrentModeUpdate, Plan, PlanEntry,
    PlanEntryPriority, PlanEntryStatus, SessionConfigOption, SessionConfigOptionCategory,
    SessionConfigSelectOption, SessionModeId, SessionNotification, SessionUpdate, ToolCall,
    ToolCallContent, ToolCallId, ToolCallLocation, ToolCallStatus, ToolCallUpdate,
    ToolCallUpdateFields, ToolKind, UsageUpdate,
};

/// Fallback context-window size (tokens) used when the runner does not report one.
/// Matches the default Claude context window.
const DEFAULT_CONTEXT_WINDOW: u64 = 200_000;

/// Available permission modes exposed in `ConfigOptionUpdate` after a mode change.
const MODE_OPTIONS: &[(&str, &str)] = &[
    ("default", "Default"),
    ("acceptEdits", "Accept Edits"),
    ("plan", "Plan Mode"),
    ("dontAsk", "Don't Ask"),
];

/// Available models exposed in `ConfigOptionUpdate` after a mode change.
const MODEL_OPTIONS: &[(&str, &str)] = &[
    ("claude-opus-4-6", "Claude Opus 4"),
    ("claude-sonnet-4-6", "Claude Sonnet 4"),
    ("claude-haiku-4-5-20251001", "Claude Haiku 4.5"),
];

/// Final outcome of a prompt run published to `ext_session_prompt_response`.
pub enum PromptOutcome {
    /// The turn finished normally.
    Done { stop_reason: String },
    /// The runner encountered an unrecoverable error.
    Error { message: String },
}

/// Converts a sequence of `PromptEvent`s (runner wire format) into
/// `SessionNotification`s (ACP wire format) and a final `PromptOutcome`.
///
/// Maintains stateful caches for tool deduplication and tool name/input lookups.
pub struct PromptEventConverter {
    session_id: String,
    /// id → (name, input) for tools that have started but not yet finished.
    tool_cache: HashMap<String, (String, serde_json::Value)>,
    /// IDs of TodoWrite tool calls (finished event is silent).
    todo_ids: HashSet<String>,
    /// IDs we have already emitted a ToolCall notification for (dedup).
    seen_tool_ids: HashSet<String>,
}

impl PromptEventConverter {
    #[cfg_attr(coverage, coverage(off))]
    pub fn new(session_id: impl Into<String>) -> Self {
        Self {
            session_id: session_id.into(),
            tool_cache: HashMap::new(),
            todo_ids: HashSet::new(),
            seen_tool_ids: HashSet::new(),
        }
    }

    /// Convert one `PromptEvent` into notifications and an optional final outcome.
    ///
    /// Returns `(notifications, outcome)`. When `outcome` is `Some`, this is the
    /// last event and no more events should be processed.
    #[cfg_attr(coverage, coverage(off))]
    pub fn convert(
        &mut self,
        event: PromptEvent,
    ) -> (Vec<SessionNotification>, Option<PromptOutcome>) {
        match event {
            PromptEvent::TextDelta { text } => {
                let notif = self.notif(SessionUpdate::AgentMessageChunk(ContentChunk::new(
                    ContentBlock::from(text),
                )));
                (vec![notif], None)
            }

            PromptEvent::ThinkingDelta { text } => {
                let notif = self.notif(SessionUpdate::AgentThoughtChunk(ContentChunk::new(
                    ContentBlock::from(text),
                )));
                (vec![notif], None)
            }

            PromptEvent::Done { stop_reason } => {
                (vec![], Some(PromptOutcome::Done { stop_reason }))
            }

            PromptEvent::Error { message } => (vec![], Some(PromptOutcome::Error { message })),

            PromptEvent::UsageUpdate {
                input_tokens,
                output_tokens,
                context_window,
                ..
            } => {
                let used = (input_tokens + output_tokens) as u64;
                let size = context_window.unwrap_or(DEFAULT_CONTEXT_WINDOW);
                let notif = self.notif(SessionUpdate::UsageUpdate(UsageUpdate::new(used, size)));
                (vec![notif], None)
            }

            PromptEvent::ModeChanged { mode, model } => {
                let mode_notif = self.notif(SessionUpdate::CurrentModeUpdate(
                    CurrentModeUpdate::new(SessionModeId::from(mode.clone())),
                ));
                let cfg_notif = self.notif(SessionUpdate::ConfigOptionUpdate(
                    ConfigOptionUpdate::new(build_plan_mode_config_options(&mode, &model)),
                ));
                (vec![mode_notif, cfg_notif], None)
            }

            PromptEvent::SystemStatus { message } => {
                let text = system_status_to_text(&message);
                match text {
                    Some(t) => {
                        let notif = self.notif(SessionUpdate::AgentMessageChunk(
                            ContentChunk::new(ContentBlock::from(t)),
                        ));
                        (vec![notif], None)
                    }
                    None => (vec![], None),
                }
            }

            PromptEvent::ToolCallStarted {
                id,
                name,
                input,
                parent_tool_use_id,
            } => {
                // Deduplicate: skip if we've already emitted a ToolCall for this ID.
                if !self.seen_tool_ids.insert(id.clone()) {
                    return (vec![], None);
                }

                if name == "TodoWrite" {
                    self.todo_ids.insert(id.clone());
                    let entries = todo_write_to_plan_entries(&input).unwrap_or_default();
                    let notif = self.notif(SessionUpdate::Plan(Plan::new(entries)));
                    return (vec![notif], None);
                }

                self.tool_cache
                    .insert(id.clone(), (name.clone(), input.clone()));

                let kind = tool_kind_for(&name);
                let locations = tool_locations_from_input(&name, &input);
                let meta = build_tool_call_meta(&name, parent_tool_use_id.as_deref());

                let tool_call = ToolCall::new(ToolCallId::new(id), &name)
                    .kind(kind)
                    .status(ToolCallStatus::InProgress)
                    .locations(locations)
                    .raw_input(input)
                    .meta(meta);

                let notif = self.notif(SessionUpdate::ToolCall(tool_call));
                (vec![notif], None)
            }

            PromptEvent::ToolCallFinished {
                id,
                output,
                exit_code,
                signal,
            } => {
                // TodoWrite finish is silent.
                if self.todo_ids.contains(&id) {
                    return (vec![], None);
                }

                let status = if exit_code == Some(0) && signal.is_none() {
                    ToolCallStatus::Completed
                } else {
                    ToolCallStatus::Failed
                };

                let (content, locations) = if let Some((name, input)) = self.tool_cache.get(&id) {
                    tool_result_content(name, input, &output, status)
                } else {
                    (vec![], vec![])
                };

                let meta = self
                    .tool_cache
                    .get(&id)
                    .and_then(|(name, _)| build_tool_call_meta(name, None));

                let fields = ToolCallUpdateFields::new()
                    .status(status)
                    .content(if content.is_empty() {
                        None
                    } else {
                        Some(content)
                    })
                    .locations(if locations.is_empty() {
                        None
                    } else {
                        Some(locations)
                    })
                    .raw_output(serde_json::Value::String(output));

                let update = ToolCallUpdate::new(ToolCallId::new(id), fields).meta(meta);
                let notif = self.notif(SessionUpdate::ToolCallUpdate(update));
                (vec![notif], None)
            }
        }
    }

    #[cfg_attr(coverage, coverage(off))]
    fn notif(&self, update: SessionUpdate) -> SessionNotification {
        SessionNotification::new(self.session_id.clone(), update)
    }
}

// ── Helper functions ──────────────────────────────────────────────────────────

#[cfg_attr(coverage, coverage(off))]
fn system_status_to_text(message: &str) -> Option<String> {
    let lower = message.to_lowercase();
    if lower.contains("compact complete") || lower.contains("compacting complete") {
        Some("\n\nCompacting completed.".to_string())
    } else if lower.contains("compact") {
        Some("Compacting...".to_string())
    } else {
        None
    }
}

#[cfg_attr(coverage, coverage(off))]
fn tool_kind_for(name: &str) -> ToolKind {
    match name {
        "Read" | "LS" => ToolKind::Read,
        "Edit" | "MultiEdit" | "Write" | "NotebookEdit" => ToolKind::Edit,
        "Bash" => ToolKind::Execute,
        "Glob" | "Grep" => ToolKind::Search,
        "WebSearch" | "WebFetch" => ToolKind::Fetch,
        "Think" => ToolKind::Think,
        "ExitPlanMode" | "EnterPlanMode" => ToolKind::SwitchMode,
        _ => ToolKind::Other,
    }
}

#[cfg_attr(coverage, coverage(off))]
fn tool_locations_from_input(name: &str, input: &serde_json::Value) -> Vec<ToolCallLocation> {
    let path_key = match name {
        "Read" | "Edit" | "MultiEdit" | "Write" | "NotebookEdit" => "file_path",
        "Glob" | "Grep" => "path",
        _ => return vec![],
    };
    if let Some(p) = input.get(path_key).and_then(|v| v.as_str()) {
        vec![ToolCallLocation::new(p)]
    } else {
        vec![]
    }
}

#[cfg_attr(coverage, coverage(off))]
fn build_tool_call_meta(
    tool_name: &str,
    parent_tool_use_id: Option<&str>,
) -> Option<agent_client_protocol::Meta> {
    let mut claude_code = serde_json::Map::new();
    claude_code.insert(
        "toolName".to_string(),
        serde_json::Value::String(tool_name.to_string()),
    );
    if let Some(parent_id) = parent_tool_use_id {
        claude_code.insert(
            "parentToolUseId".to_string(),
            serde_json::Value::String(parent_id.to_string()),
        );
    }
    let mut meta = serde_json::Map::new();
    meta.insert(
        "claudeCode".to_string(),
        serde_json::Value::Object(claude_code),
    );
    Some(meta)
}

#[cfg_attr(coverage, coverage(off))]
fn todo_write_to_plan_entries(input: &serde_json::Value) -> Option<Vec<PlanEntry>> {
    let todos = input.get("todos")?.as_array()?;
    let entries: Vec<PlanEntry> = todos
        .iter()
        .filter_map(|todo| {
            let content = todo.get("content")?.as_str()?.to_string();
            let status = match todo.get("status").and_then(|v| v.as_str()) {
                Some("in_progress") => PlanEntryStatus::InProgress,
                Some("completed") => PlanEntryStatus::Completed,
                _ => PlanEntryStatus::Pending,
            };
            let priority = match todo.get("priority").and_then(|v| v.as_str()) {
                Some("medium") => PlanEntryPriority::Medium,
                Some("low") => PlanEntryPriority::Low,
                _ => PlanEntryPriority::High,
            };
            Some(PlanEntry::new(content, priority, status))
        })
        .collect();
    if entries.is_empty() {
        None
    } else {
        Some(entries)
    }
}

#[cfg_attr(coverage, coverage(off))]
fn build_plan_mode_config_options(mode: &str, model: &str) -> Vec<SessionConfigOption> {
    let mode_options: Vec<SessionConfigSelectOption> = MODE_OPTIONS
        .iter()
        .map(|(value, name)| SessionConfigSelectOption::new(*value, *name))
        .collect();
    let model_options: Vec<SessionConfigSelectOption> = MODEL_OPTIONS
        .iter()
        .map(|(value, name)| SessionConfigSelectOption::new(*value, *name))
        .collect();
    vec![
        SessionConfigOption::select("mode", "Mode", mode.to_string(), mode_options)
            .category(SessionConfigOptionCategory::Mode),
        SessionConfigOption::select("model", "Model", model.to_string(), model_options)
            .category(SessionConfigOptionCategory::Model),
    ]
}

/// Wrap text in a fenced code block.
#[cfg_attr(coverage, coverage(off))]
fn markdown_fence(text: &str) -> String {
    let mut fence = "```".to_string();
    for cap in text.lines().filter(|l| l.starts_with("```")) {
        while cap.len() >= fence.len() {
            fence.push('`');
        }
    }
    format!(
        "{fence}\n{}{}\n{fence}",
        text,
        if text.ends_with('\n') { "" } else { "\n" }
    )
}

#[cfg_attr(coverage, coverage(off))]
fn tool_result_content(
    tool_name: &str,
    input: &serde_json::Value,
    output: &str,
    status: ToolCallStatus,
) -> (Vec<ToolCallContent>, Vec<ToolCallLocation>) {
    match tool_name {
        "Edit" | "MultiEdit" => {
            let file_path = input.get("file_path").and_then(|v| v.as_str());
            let Some(file_path) = file_path else {
                return (vec![], vec![]);
            };
            // Collect (old, new) pairs
            let pairs: Vec<(Option<&str>, &str)> = if tool_name == "MultiEdit" {
                input
                    .get("edits")
                    .and_then(|v| v.as_array())
                    .map(|edits| {
                        edits
                            .iter()
                            .filter_map(|e| {
                                let new = e.get("new_string")?.as_str()?;
                                let old = e.get("old_string").and_then(|v| v.as_str());
                                Some((old, new))
                            })
                            .collect()
                    })
                    .unwrap_or_default()
            } else {
                let new = input.get("new_string").and_then(|v| v.as_str());
                let old = input.get("old_string").and_then(|v| v.as_str());
                match new {
                    Some(n) => vec![(old, n)],
                    None => vec![],
                }
            };

            if pairs.is_empty() {
                return (vec![], vec![]);
            }

            let content: Vec<ToolCallContent> = if status == ToolCallStatus::Completed {
                pairs
                    .into_iter()
                    .map(|(old, new)| {
                        let mut diff = agent_client_protocol::Diff::new(file_path, new);
                        if let Some(old_text) = old {
                            diff = diff.old_text(old_text.to_string());
                        }
                        ToolCallContent::from(diff)
                    })
                    .collect()
            } else {
                vec![]
            };
            let locations = vec![ToolCallLocation::new(file_path)];
            (content, locations)
        }

        "Write" => {
            let file_path = input.get("file_path").and_then(|v| v.as_str());
            let Some(file_path) = file_path else {
                return (vec![], vec![]);
            };
            let content: Vec<ToolCallContent> = if status == ToolCallStatus::Completed {
                let new_text = input
                    .get("content")
                    .and_then(|v| v.as_str())
                    .unwrap_or(output);
                vec![ToolCallContent::from(agent_client_protocol::Diff::new(
                    file_path, new_text,
                ))]
            } else {
                vec![]
            };
            let locations = vec![ToolCallLocation::new(file_path)];
            (content, locations)
        }

        "Read" => {
            let content = if status == ToolCallStatus::Completed {
                let fenced = markdown_fence(output);
                vec![ToolCallContent::from(ContentBlock::from(fenced))]
            } else {
                vec![]
            };
            let locations = tool_locations_from_input(tool_name, input);
            (content, locations)
        }

        _ => (vec![], vec![]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── tool_kind_for ─────────────────────────────────────────────────────────

    #[test]
    fn tool_kind_for_read_tools() {
        assert!(matches!(tool_kind_for("Read"), ToolKind::Read));
        assert!(matches!(tool_kind_for("LS"), ToolKind::Read));
    }

    #[test]
    fn tool_kind_for_edit_tools() {
        assert!(matches!(tool_kind_for("Edit"), ToolKind::Edit));
        assert!(matches!(tool_kind_for("MultiEdit"), ToolKind::Edit));
        assert!(matches!(tool_kind_for("Write"), ToolKind::Edit));
        assert!(matches!(tool_kind_for("NotebookEdit"), ToolKind::Edit));
    }

    #[test]
    fn tool_kind_for_bash_is_execute() {
        assert!(matches!(tool_kind_for("Bash"), ToolKind::Execute));
    }

    // ── tool_locations_from_input ─────────────────────────────────────────────

    #[test]
    fn tool_locations_from_input_returns_location_for_file_path_tools() {
        let input = serde_json::json!({"file_path": "/src/main.rs"});
        for tool in &["Read", "Edit", "MultiEdit", "Write", "NotebookEdit"] {
            let locs = tool_locations_from_input(tool, &input);
            assert_eq!(locs.len(), 1, "expected 1 location for {tool}");
        }
    }

    #[test]
    fn tool_locations_from_input_returns_location_for_glob_and_grep() {
        let input = serde_json::json!({"path": "/src"});
        assert_eq!(tool_locations_from_input("Glob", &input).len(), 1);
        assert_eq!(tool_locations_from_input("Grep", &input).len(), 1);
    }

    #[test]
    fn tool_locations_from_input_returns_empty_for_unknown_tool() {
        let input = serde_json::json!({"file_path": "/src/main.rs"});
        let locs = tool_locations_from_input("Bash", &input);
        assert!(locs.is_empty(), "Bash has no location extraction");
    }

    // ── markdown_fence ────────────────────────────────────────────────────────

    #[test]
    fn markdown_fence_plain_text_uses_triple_backtick() {
        let fenced = markdown_fence("hello world");
        assert!(fenced.starts_with("```\n"), "expected ```, got: {fenced}");
        assert!(
            fenced.ends_with("\n```"),
            "expected trailing ```, got: {fenced}"
        );
        assert!(fenced.contains("hello world"));
    }

    #[test]
    fn markdown_fence_text_ending_with_newline_no_triple_newline() {
        let fenced = markdown_fence("line\n");
        assert!(
            !fenced.contains("\n\n\n```"),
            "should not triple-newline when text ends with newline, got: {fenced:?}"
        );
        assert!(
            fenced.ends_with("\n```"),
            "must end with closing fence, got: {fenced:?}"
        );
    }
}

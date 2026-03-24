use std::collections::{HashMap, HashSet};

use agent_client_protocol::{
    ConfigOptionUpdate, ContentBlock, ContentChunk, CurrentModeUpdate, Plan, PlanEntry,
    PlanEntryPriority, PlanEntryStatus, SessionConfigOption, SessionConfigOptionCategory,
    SessionConfigSelectOption, SessionModeId, SessionNotification, SessionUpdate,
    ToolCall, ToolCallContent, ToolCallId, ToolCallLocation, ToolCallStatus, ToolCallUpdate,
    ToolCallUpdateFields, ToolKind, UsageUpdate,
};
use serde::{Deserialize, Serialize};

/// A rich content block transported over NATS from Bridge to Runner.
///
/// Mirrors the ACP `ContentBlock` variants we care about, in a compact wire format.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum UserContentBlock {
    /// Plain text.
    Text { text: String },
    /// Base64-encoded image.
    Image { data: String, mime_type: String },
    /// HTTP/HTTPS image URL (passed natively to the Anthropic API as a URL image source).
    ImageUrl { url: String },
    /// Reference link to a resource (shown as `[@name](uri)`).
    ResourceLink { uri: String, name: String },
    /// Embedded text resource (shown as XML context block).
    Context { uri: String, text: String },
}

/// Payload published by the Bridge to NATS when it receives a prompt from an ACP client.
///
/// Subject: `{prefix}.{session_id}.agent.prompt`
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromptPayload {
    /// Unique request ID — used to route events back to the calling Bridge instance.
    pub req_id: String,
    /// The ACP session ID.
    pub session_id: String,
    /// Rich content blocks from the ACP prompt (text, images, resources).
    /// Always populated by current Bridge versions.
    pub content: Vec<UserContentBlock>,
    /// Plain-text fallback for backward compatibility.
    /// Used only when `content` is empty (old Bridge versions).
    #[serde(default)]
    pub user_message: String,
}

/// Events published by the Runner back to the Bridge for a specific prompt request.
///
/// Subject: `{prefix}.{session_id}.agent.prompt.events.{req_id}`
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PromptEvent {
    /// A chunk of text produced by the model.
    TextDelta { text: String },
    /// A chunk of the model's internal reasoning (extended thinking).
    ThinkingDelta { text: String },
    /// The runner finished the turn. `stop_reason` matches Anthropic values:
    /// `"end_turn"`, `"max_tokens"`, `"max_turn_requests"`, `"cancelled"`.
    Done { stop_reason: String },
    /// The runner encountered an unrecoverable error.
    Error { message: String },
    /// A tool call was dispatched to the tool executor.
    ToolCallStarted {
        id: String,
        name: String,
        input: serde_json::Value,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        parent_tool_use_id: Option<String>,
    },
    /// A tool call finished executing.
    ToolCallFinished {
        id: String,
        output: String,
        #[serde(default)]
        exit_code: Option<i32>,
        #[serde(default)]
        signal: Option<String>,
    },
    /// A system-level status message (forward compatibility with Anthropic API system events).
    SystemStatus { message: String },
    /// Token usage summary for the completed turn.
    UsageUpdate {
        input_tokens: u32,
        output_tokens: u32,
        #[serde(default)]
        cache_creation_tokens: u32,
        #[serde(default)]
        cache_read_tokens: u32,
        /// Context window size for the model being used (if known).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        context_window: Option<u64>,
    },
    /// The agent entered plan mode via the `EnterPlanMode` tool.
    /// Carries the new mode name and the active model so the Bridge can build
    /// the full `ConfigOptionUpdate` without access to the ACP agent's config.
    ModeChanged { mode: String, model: String },
}

// ── Converter ─────────────────────────────────────────────────────────────────

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

            PromptEvent::Error { message } => {
                (vec![], Some(PromptOutcome::Error { message }))
            }

            PromptEvent::UsageUpdate {
                input_tokens,
                output_tokens,
                context_window,
                ..
            } => {
                let used = (input_tokens + output_tokens) as u64;
                let size = context_window.unwrap_or(200_000);
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

                self.tool_cache.insert(id.clone(), (name.clone(), input.clone()));

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

                let meta = self.tool_cache.get(&id).and_then(|(name, _)| {
                    build_tool_call_meta(name, None)
                });

                let fields = ToolCallUpdateFields::new()
                    .status(status)
                    .content(if content.is_empty() { None } else { Some(content) })
                    .locations(if locations.is_empty() { None } else { Some(locations) })
                    .raw_output(serde_json::Value::String(output));

                let update = ToolCallUpdate::new(ToolCallId::new(id), fields).meta(meta);
                let notif = self.notif(SessionUpdate::ToolCallUpdate(update));
                (vec![notif], None)
            }
        }
    }

    fn notif(&self, update: SessionUpdate) -> SessionNotification {
        SessionNotification::new(self.session_id.clone(), update)
    }
}

// ── Helper functions ──────────────────────────────────────────────────────────

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
    if entries.is_empty() { None } else { Some(entries) }
}

fn build_plan_mode_config_options(mode: &str, model: &str) -> Vec<SessionConfigOption> {
    let mode_options = vec![
        SessionConfigSelectOption::new("default", "Default"),
        SessionConfigSelectOption::new("acceptEdits", "Accept Edits"),
        SessionConfigSelectOption::new("plan", "Plan Mode"),
        SessionConfigSelectOption::new("dontAsk", "Don't Ask"),
    ];
    let model_options = vec![
        SessionConfigSelectOption::new("claude-opus-4-6", "Claude Opus 4"),
        SessionConfigSelectOption::new("claude-sonnet-4-6", "Claude Sonnet 4"),
        SessionConfigSelectOption::new("claude-haiku-4-5-20251001", "Claude Haiku 4.5"),
    ];
    vec![
        SessionConfigOption::select("mode", "Mode", mode.to_string(), mode_options)
            .category(SessionConfigOptionCategory::Mode),
        SessionConfigOption::select("model", "Model", model.to_string(), model_options)
            .category(SessionConfigOptionCategory::Model),
    ]
}

/// Wrap text in a fenced code block.
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

    #[test]
    fn prompt_payload_roundtrip() {
        let p = PromptPayload {
            req_id: "req-1".to_string(),
            session_id: "sess-1".to_string(),
            content: vec![],
            user_message: "hello".to_string(),
        };
        let json = serde_json::to_string(&p).unwrap();
        let p2: PromptPayload = serde_json::from_str(&json).unwrap();
        assert_eq!(p2.req_id, "req-1");
        assert_eq!(p2.session_id, "sess-1");
        assert_eq!(p2.user_message, "hello");
    }

    #[test]
    fn prompt_event_text_delta_tag() {
        let e = PromptEvent::TextDelta {
            text: "hi".to_string(),
        };
        let v = serde_json::to_value(&e).unwrap();
        assert_eq!(v["type"], "text_delta");
        assert_eq!(v["text"], "hi");
    }

    #[test]
    fn prompt_event_done_tag() {
        let e = PromptEvent::Done {
            stop_reason: "end_turn".to_string(),
        };
        let v = serde_json::to_value(&e).unwrap();
        assert_eq!(v["type"], "done");
        assert_eq!(v["stop_reason"], "end_turn");
    }

    #[test]
    fn prompt_event_error_tag() {
        let e = PromptEvent::Error {
            message: "oops".to_string(),
        };
        let v = serde_json::to_value(&e).unwrap();
        assert_eq!(v["type"], "error");
        assert_eq!(v["message"], "oops");
    }

    #[test]
    fn prompt_event_usage_update_tag() {
        let e = PromptEvent::UsageUpdate {
            input_tokens: 100,
            output_tokens: 50,
            cache_creation_tokens: 0,
            cache_read_tokens: 0,
            context_window: None,
        };
        let v = serde_json::to_value(&e).unwrap();
        assert_eq!(v["type"], "usage_update");
        assert_eq!(v["input_tokens"], 100);
        assert_eq!(v["output_tokens"], 50);
    }

    #[test]
    fn prompt_event_roundtrip_done() {
        let e = PromptEvent::Done {
            stop_reason: "end_turn".to_string(),
        };
        let json = serde_json::to_string(&e).unwrap();
        let e2: PromptEvent = serde_json::from_str(&json).unwrap();
        assert!(matches!(e2, PromptEvent::Done { stop_reason } if stop_reason == "end_turn"));
    }

    #[test]
    fn prompt_event_system_status_tag() {
        let e = PromptEvent::SystemStatus {
            message: "rate_limit_warning".to_string(),
        };
        let v = serde_json::to_value(&e).unwrap();
        assert_eq!(v["type"], "system_status");
        assert_eq!(v["message"], "rate_limit_warning");
        // Roundtrip
        let json = serde_json::to_string(&e).unwrap();
        let e2: PromptEvent = serde_json::from_str(&json).unwrap();
        assert!(
            matches!(e2, PromptEvent::SystemStatus { message } if message == "rate_limit_warning")
        );
    }

    #[test]
    fn prompt_event_mode_changed_tag() {
        let e = PromptEvent::ModeChanged {
            mode: "plan".to_string(),
            model: "claude-opus-4-6".to_string(),
        };
        let v = serde_json::to_value(&e).unwrap();
        assert_eq!(v["type"], "mode_changed");
        assert_eq!(v["mode"], "plan");
        assert_eq!(v["model"], "claude-opus-4-6");
    }

    #[test]
    fn prompt_event_mode_changed_roundtrip() {
        let e = PromptEvent::ModeChanged {
            mode: "plan".to_string(),
            model: "claude-sonnet-4-6".to_string(),
        };
        let json = serde_json::to_string(&e).unwrap();
        let e2: PromptEvent = serde_json::from_str(&json).unwrap();
        assert!(
            matches!(e2, PromptEvent::ModeChanged { ref mode, ref model }
                if mode == "plan" && model == "claude-sonnet-4-6")
        );
    }

    #[test]
    fn prompt_event_mode_changed_deserialize_from_wire() {
        // Verify the exact wire format the runner publishes can be decoded by the bridge
        let wire = r#"{"type":"mode_changed","mode":"plan","model":"claude-opus-4-6"}"#;
        let e: PromptEvent = serde_json::from_str(wire).unwrap();
        assert!(matches!(e, PromptEvent::ModeChanged { ref mode, .. } if mode == "plan"));
    }

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

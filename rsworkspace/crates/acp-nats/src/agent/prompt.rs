use crate::prompt_event::UserContentBlock;
use agent_client_protocol::{
    ConfigOptionUpdate, ContentBlock, ContentChunk, CurrentModeUpdate, Diff,
    EmbeddedResourceResource, Error, ErrorCode, Plan, PlanEntry, PlanEntryPriority,
    PlanEntryStatus, PromptRequest, PromptResponse, SessionConfigOption,
    SessionConfigOptionCategory, SessionConfigSelectOption, SessionNotification, SessionUpdate,
    StopReason, TextContent, ToolCall, ToolCallContent, ToolCallLocation, ToolCallStatus,
    ToolCallUpdate, ToolCallUpdateFields, ToolKind, Usage, UsageUpdate,
};
use bytes::Bytes;
use futures_util::StreamExt;
use tokio::time::timeout;
use tracing::warn;

use crate::agent::Bridge;
use crate::nats::{FlushClient, PublishClient, RequestClient, SubscribeClient, agent};
use crate::prompt_event::{PromptEvent, PromptPayload};
use crate::session_id::AcpSessionId;

pub async fn handle<N, C>(
    bridge: &Bridge<N, C>,
    args: PromptRequest,
) -> agent_client_protocol::Result<PromptResponse>
where
    N: RequestClient + PublishClient + SubscribeClient + FlushClient,
    C: trogon_std::time::GetElapsed,
{
    // 1. Validate session ID — reject before touching NATS
    let session_id = AcpSessionId::try_from(&args.session_id)
        .map_err(|_| Error::new(ErrorCode::InvalidParams.into(), "Invalid session ID"))?;

    // 2. Convert ACP content blocks to rich UserContentBlocks for the runner
    let content = acp_blocks_to_user_content(&args.prompt);

    // Plain-text fallback (used for title, backward compat)
    let user_message = args
        .prompt
        .iter()
        .filter_map(|block| {
            if let ContentBlock::Text(t) = block {
                Some(t.text.as_str())
            } else {
                None
            }
        })
        .collect::<Vec<_>>()
        .join("\n");

    // 3. Generate a unique request ID to correlate the response event stream
    let req_id = uuid::Uuid::new_v4().to_string();

    // 4. Subscribe to events BEFORE publishing the prompt — prevents losing the
    //    first event in case the runner is already running and responds instantly
    let events_subject =
        agent::prompt_events(bridge.config.acp_prefix(), session_id.as_ref(), &req_id);

    let mut subscriber = bridge
        .nats
        .subscribe(events_subject)
        .await
        .map_err(|e| Error::new(ErrorCode::InternalError.into(), format!("subscribe: {e}")))?;

    let session_cancelled_subject =
        agent::session_cancelled(bridge.config.acp_prefix(), session_id.as_ref());
    let mut cancel_notify = bridge
        .nats
        .subscribe(session_cancelled_subject)
        .await
        .map_err(|e| {
            Error::new(
                ErrorCode::InternalError.into(),
                format!("subscribe cancelled: {e}"),
            )
        })?;

    // 5. Build and publish the prompt payload via NATS Core
    let payload = PromptPayload {
        req_id,
        session_id: session_id.to_string(),
        content,
        user_message,
    };
    let payload_bytes = serde_json::to_vec(&payload)
        .map_err(|e| Error::new(ErrorCode::InternalError.into(), e.to_string()))?;

    let prompt_subject = agent::prompt(bridge.config.acp_prefix(), session_id.as_ref());
    bridge
        .nats
        .publish_with_headers(
            prompt_subject,
            async_nats::HeaderMap::new(),
            Bytes::from(payload_bytes),
        )
        .await
        .map_err(|e| Error::new(ErrorCode::InternalError.into(), format!("publish: {e}")))?;

    bridge
        .nats
        .flush()
        .await
        .map_err(|e| Error::new(ErrorCode::InternalError.into(), format!("flush: {e}")))?;

    // 6. Stream events back to the ACP client until the runner signals Done
    // Prompts may be queued behind other running prompts — use a long timeout
    // so queued prompts don't time out while waiting for the runner.
    let op_timeout = std::time::Duration::from_secs(600); // 10 minutes

    let mut accumulated_input: u64 = 0;
    let mut accumulated_output: u64 = 0;
    let mut accumulated_cache_creation: u64 = 0;
    let mut accumulated_cache_read: u64 = 0;
    let mut seen_tool_ids: std::collections::HashSet<String> = std::collections::HashSet::new();
    // id → tool name cache for _meta.claudeCode.toolName on ToolCallFinished
    let mut tool_name_cache: std::collections::HashMap<String, String> =
        std::collections::HashMap::new();
    // id → tool input cache — used for Edit diffs and location extraction on finish
    let mut tool_input_cache: std::collections::HashMap<String, serde_json::Value> =
        std::collections::HashMap::new();
    // ids of TodoWrite calls — emitted as Plan updates, no ToolCallUpdate on finish
    let mut todo_write_ids: std::collections::HashSet<String> = std::collections::HashSet::new();

    let supports_terminal = bridge.supports_terminal_output();

    loop {
        let msg = tokio::select! {
            result = timeout(op_timeout, subscriber.next()) => {
                match result {
                    Ok(Some(msg)) => msg,
                    Ok(None) => {
                        return Err(Error::new(
                            ErrorCode::InternalError.into(),
                            "prompt event stream closed unexpectedly",
                        ));
                    }
                    Err(_elapsed) => {
                        return Err(Error::new(
                            ErrorCode::InternalError.into(),
                            "prompt timed out waiting for runner",
                        ));
                    }
                }
            }
            _ = cancel_notify.next() => {
                let total = accumulated_input + accumulated_output
                    + accumulated_cache_creation + accumulated_cache_read;
                let usage = Usage::new(total, accumulated_input, accumulated_output)
                    .cached_read_tokens(accumulated_cache_read)
                    .cached_write_tokens(accumulated_cache_creation);
                return Ok(PromptResponse::new(StopReason::Cancelled).usage(usage));
            }
        };

        let event: PromptEvent = match serde_json::from_slice(&msg.payload) {
            Ok(e) => e,
            Err(e) => {
                return Err(Error::new(
                    ErrorCode::InternalError.into(),
                    format!("bad event payload: {e}"),
                ));
            }
        };

        match event {
            PromptEvent::TextDelta { text } => {
                let notification = SessionNotification::new(
                    args.session_id.clone(),
                    SessionUpdate::AgentMessageChunk(ContentChunk::new(ContentBlock::Text(
                        TextContent::new(text),
                    ))),
                );
                // Best-effort: if the receiver was dropped the turn still completes
                if bridge.notification_sender.send(notification).await.is_err() {
                    warn!("notification receiver dropped; continuing prompt");
                }
            }
            PromptEvent::ThinkingDelta { text } => {
                let notification = SessionNotification::new(
                    args.session_id.clone(),
                    SessionUpdate::AgentThoughtChunk(ContentChunk::new(ContentBlock::Text(
                        TextContent::new(text),
                    ))),
                );
                if bridge.notification_sender.send(notification).await.is_err() {
                    warn!("notification receiver dropped; continuing prompt");
                }
            }
            PromptEvent::Done { stop_reason } => {
                let sr = match stop_reason.as_str() {
                    "end_turn" => StopReason::EndTurn,
                    "max_tokens" => StopReason::MaxTokens,
                    "max_turn_requests" => StopReason::MaxTurnRequests,
                    "cancelled" => StopReason::Cancelled,
                    other => {
                        warn!(stop_reason = other, "unknown stop reason — using EndTurn");
                        StopReason::EndTurn
                    }
                };
                let total = accumulated_input
                    + accumulated_output
                    + accumulated_cache_creation
                    + accumulated_cache_read;
                let usage = Usage::new(total, accumulated_input, accumulated_output)
                    .cached_read_tokens(accumulated_cache_read)
                    .cached_write_tokens(accumulated_cache_creation);
                return Ok(PromptResponse::new(sr).usage(usage));
            }
            PromptEvent::Error { message } => {
                return Err(Error::new(ErrorCode::InternalError.into(), message));
            }
            PromptEvent::ToolCallStarted { id, name, input } => {
                if seen_tool_ids.contains(&id) {
                    continue;
                }
                seen_tool_ids.insert(id.clone());
                tool_name_cache.insert(id.clone(), name.clone());
                tool_input_cache.insert(id.clone(), input.clone());

                // TodoWrite → emit Plan update instead of a tool_call notification
                if name == "TodoWrite"
                    && let Some(entries) = todo_write_to_plan_entries(&input)
                {
                    todo_write_ids.insert(id);
                    let notification = SessionNotification::new(
                        args.session_id.clone(),
                        SessionUpdate::Plan(Plan::new(entries)),
                    );
                    if bridge.notification_sender.send(notification).await.is_err() {
                        warn!("notification receiver dropped; continuing prompt");
                    }
                    continue;
                }

                let meta = if name == "Bash" && supports_terminal {
                    make_meta_with_terminal_info(&name, &id)
                } else {
                    make_claude_code_meta(&name)
                };
                let kind = tool_kind_for(&name);
                let locations = tool_locations_from_input(&name, &input);
                let tool_call = ToolCall::new(id, name.clone())
                    .status(ToolCallStatus::InProgress)
                    .kind(kind)
                    .locations(locations)
                    .raw_input(input)
                    .meta(meta);
                let notification = SessionNotification::new(
                    args.session_id.clone(),
                    SessionUpdate::ToolCall(tool_call),
                );
                if bridge.notification_sender.send(notification).await.is_err() {
                    warn!("notification receiver dropped; continuing prompt");
                }
            }
            PromptEvent::ToolCallFinished {
                id,
                output,
                exit_code,
                signal,
            } => {
                // TodoWrite finished — Plan was already sent, skip the tool_call_update
                if todo_write_ids.contains(&id) {
                    continue;
                }

                let is_bash = tool_name_cache.get(&id).is_some_and(|n| n == "Bash");

                // When terminal_output is supported, emit two separate tool_call_update
                // notifications for Bash tools — matching Zed's codex-acp lifecycle:
                //   1. terminal_output  → streams the captured stdout/stderr
                //   2. terminal_exit    → final status with exit code / signal
                if is_bash && supports_terminal {
                    // 1. terminal_output notification
                    let output_meta = make_meta_with_terminal_output(
                        tool_name_cache
                            .get(&id)
                            .map(String::as_str)
                            .unwrap_or("Bash"),
                        &id,
                        &output,
                    );
                    let output_fields = ToolCallUpdateFields::new();
                    let output_update =
                        ToolCallUpdate::new(id.clone(), output_fields).meta(output_meta);
                    let output_notification = SessionNotification::new(
                        args.session_id.clone(),
                        SessionUpdate::ToolCallUpdate(output_update),
                    );
                    if bridge
                        .notification_sender
                        .send(output_notification)
                        .await
                        .is_err()
                    {
                        warn_notification_dropped();
                    }

                    // 2. terminal_exit notification (also carries final status + raw_output)
                    let status = if exit_code.map(|c| c != 0).unwrap_or(false) || signal.is_some() {
                        ToolCallStatus::Failed
                    } else {
                        ToolCallStatus::Completed
                    };
                    let exit_meta = make_meta_with_terminal_exit(
                        tool_name_cache
                            .get(&id)
                            .map(String::as_str)
                            .unwrap_or("Bash"),
                        &id,
                        exit_code,
                        signal.as_deref(),
                    );
                    let exit_fields = ToolCallUpdateFields::new()
                        .status(status)
                        .raw_output(serde_json::Value::String(output));
                    let exit_update = ToolCallUpdate::new(id.clone(), exit_fields).meta(exit_meta);
                    let exit_notification = SessionNotification::new(
                        args.session_id.clone(),
                        SessionUpdate::ToolCallUpdate(exit_update),
                    );
                    if bridge
                        .notification_sender
                        .send(exit_notification)
                        .await
                        .is_err()
                    {
                        warn_notification_dropped();
                    }
                    continue;
                }

                let status = if exit_code.map(|c| c != 0).unwrap_or(false) || signal.is_some() {
                    ToolCallStatus::Failed
                } else {
                    ToolCallStatus::Completed
                };
                let tool_name = tool_name_cache.get(&id).map(String::as_str).unwrap_or("");
                let cached_input = tool_input_cache.get(&id);
                let (content, locations) =
                    tool_result_content(tool_name, cached_input, &output, status);
                let fields = ToolCallUpdateFields::new()
                    .status(status)
                    .content(content)
                    .locations(locations)
                    .raw_output(serde_json::Value::String(output));
                let update = ToolCallUpdate::new(id.clone(), fields);
                let update = if !tool_name.is_empty() {
                    update.meta(make_claude_code_meta(tool_name))
                } else {
                    update
                };
                let notification = SessionNotification::new(
                    args.session_id.clone(),
                    SessionUpdate::ToolCallUpdate(update),
                );
                if bridge.notification_sender.send(notification).await.is_err() {
                    warn!("notification receiver dropped; continuing prompt");
                }
            }
            PromptEvent::ModeChanged { mode, model } => {
                let mode_notification = SessionNotification::new(
                    args.session_id.clone(),
                    SessionUpdate::CurrentModeUpdate(CurrentModeUpdate::new(mode.clone())),
                );
                if bridge
                    .notification_sender
                    .send(mode_notification)
                    .await
                    .is_err()
                {
                    warn!("notification receiver dropped; continuing prompt");
                }
                let config_options = build_plan_mode_config_options(&mode, &model);
                let config_notification = SessionNotification::new(
                    args.session_id.clone(),
                    SessionUpdate::ConfigOptionUpdate(ConfigOptionUpdate::new(config_options)),
                );
                if bridge
                    .notification_sender
                    .send(config_notification)
                    .await
                    .is_err()
                {
                    warn!("notification receiver dropped; continuing prompt");
                }
            }
            PromptEvent::SystemStatus { message } => {
                tracing::info!(message = %message, "agent system status");
            }
            PromptEvent::UsageUpdate {
                input_tokens,
                output_tokens,
                cache_creation_tokens,
                cache_read_tokens,
                context_window,
            } => {
                accumulated_input += input_tokens as u64;
                accumulated_output += output_tokens as u64;
                accumulated_cache_creation += cache_creation_tokens as u64;
                accumulated_cache_read += cache_read_tokens as u64;
                let used = accumulated_input
                    + accumulated_output
                    + accumulated_cache_read
                    + accumulated_cache_creation;
                let size = context_window.unwrap_or(200_000u64);
                let update = UsageUpdate::new(used, size);
                let notification = SessionNotification::new(
                    args.session_id.clone(),
                    SessionUpdate::UsageUpdate(update),
                );
                if bridge.notification_sender.send(notification).await.is_err() {
                    warn!("notification receiver dropped; continuing prompt");
                }
            }
        }
    }
}

/// Format a URI as an inline reference link, matching TS `formatUriAsLink`.
/// For file:// and zed:// URIs, extracts the last path segment as the display name.
/// For other URIs, uses the full URI as-is.
fn format_uri_as_link(uri: &str) -> String {
    if uri.starts_with("file://") || uri.starts_with("zed://") {
        let name = uri
            .trim_end_matches('/')
            .rsplit('/')
            .next()
            .filter(|s| !s.is_empty())
            .unwrap_or(uri);
        format!("[@{name}]({uri})")
    } else {
        uri.to_string()
    }
}

/// Convert ACP `ContentBlock`s to `UserContentBlock`s for transport over NATS.
///
/// Follows the same logic as `promptToClaude()` in the TypeScript reference:
/// - Text → plain text
/// - ResourceLink → `[@name](uri)` formatted link
/// - Resource (text) → inline [@name](uri) link + `<context ref="uri">\n{text}\n</context>` appended at the end
/// - Resource (blob) → skipped
/// - Image (base64) → image block
/// - Image (http/https url) → native URL image block; other URL schemes → `![image](url)` text link
fn acp_blocks_to_user_content(blocks: &[ContentBlock]) -> Vec<UserContentBlock> {
    let mut content: Vec<UserContentBlock> = Vec::new();
    let mut context_parts: Vec<UserContentBlock> = Vec::new();

    for block in blocks {
        if let ContentBlock::Text(t) = block {
            let text = rewrite_mcp_slash_command(&t.text);
            content.push(UserContentBlock::Text { text });
        } else if let ContentBlock::Image(img) = block {
            if !img.data.is_empty() {
                content.push(UserContentBlock::Image {
                    data: img.data.clone(),
                    mime_type: img.mime_type.clone(),
                });
            } else if let Some(uri) = &img.uri {
                if uri.starts_with("http://") || uri.starts_with("https://") {
                    content.push(UserContentBlock::ImageUrl { url: uri.clone() });
                } else {
                    content.push(UserContentBlock::Text {
                        text: format!("![image]({uri})"),
                    });
                }
            }
        } else if let ContentBlock::ResourceLink(r) = block {
            content.push(UserContentBlock::ResourceLink {
                uri: r.uri.clone(),
                name: r.name.clone(),
            });
        } else if let ContentBlock::Resource(r) = block
            && let EmbeddedResourceResource::TextResourceContents(t) = &r.resource
        {
            // Inline reference link (position marker in the message body)
            content.push(UserContentBlock::Text {
                text: format_uri_as_link(&t.uri),
            });
            context_parts.push(UserContentBlock::Context {
                uri: t.uri.clone(),
                text: t.text.clone(),
            });
            // BlobResourceContents and future resource types are silently skipped
        }
        // Audio and future content block types are silently skipped
    }

    // Append context blocks at the end, matching the TS behaviour
    content.extend(context_parts);
    content
}

/// Build `_meta: { claudeCode: { toolName: "..." } }` for tool_call / tool_call_update.
fn make_claude_code_meta(tool_name: &str) -> agent_client_protocol::Meta {
    let mut claude_code = serde_json::Map::new();
    claude_code.insert(
        "toolName".to_string(),
        serde_json::Value::String(tool_name.to_string()),
    );
    let mut meta = serde_json::Map::new();
    meta.insert(
        "claudeCode".to_string(),
        serde_json::Value::Object(claude_code),
    );
    meta
}

/// Build `_meta: { claudeCode: { toolName }, terminal_info: { terminal_id } }`.
/// Sent on the initial `tool_call` notification for Bash when client supports terminal output.
fn make_meta_with_terminal_info(tool_name: &str, terminal_id: &str) -> agent_client_protocol::Meta {
    let mut meta = make_claude_code_meta(tool_name);
    let mut terminal_info = serde_json::Map::new();
    terminal_info.insert(
        "terminal_id".to_string(),
        serde_json::Value::String(terminal_id.to_string()),
    );
    meta.insert(
        "terminal_info".to_string(),
        serde_json::Value::Object(terminal_info),
    );
    meta
}

/// Build `_meta: { claudeCode: { toolName }, terminal_output: { terminal_id, data } }`.
/// Sent as the first `tool_call_update` for a finished Bash call when client supports terminal output.
fn make_meta_with_terminal_output(
    tool_name: &str,
    terminal_id: &str,
    data: &str,
) -> agent_client_protocol::Meta {
    let mut meta = make_claude_code_meta(tool_name);
    let mut terminal_output = serde_json::Map::new();
    terminal_output.insert(
        "terminal_id".to_string(),
        serde_json::Value::String(terminal_id.to_string()),
    );
    terminal_output.insert(
        "data".to_string(),
        serde_json::Value::String(data.to_string()),
    );
    meta.insert(
        "terminal_output".to_string(),
        serde_json::Value::Object(terminal_output),
    );
    meta
}

/// Build `_meta: { claudeCode: { toolName }, terminal_exit: { terminal_id, exit_code, signal } }`.
/// Sent as the final `tool_call_update` for a finished Bash call when client supports terminal output.
fn make_meta_with_terminal_exit(
    tool_name: &str,
    terminal_id: &str,
    exit_code: Option<i32>,
    signal: Option<&str>,
) -> agent_client_protocol::Meta {
    let mut meta = make_claude_code_meta(tool_name);
    let mut terminal_exit = serde_json::Map::new();
    terminal_exit.insert(
        "terminal_id".to_string(),
        serde_json::Value::String(terminal_id.to_string()),
    );
    terminal_exit.insert(
        "exit_code".to_string(),
        serde_json::Value::Number(serde_json::Number::from(exit_code.unwrap_or(0))),
    );
    terminal_exit.insert(
        "signal".to_string(),
        match signal {
            Some(s) => serde_json::Value::String(s.to_string()),
            None => serde_json::Value::Null,
        },
    );
    meta.insert(
        "terminal_exit".to_string(),
        serde_json::Value::Object(terminal_exit),
    );
    meta
}

/// Map a Claude Code tool name to the matching ACP `ToolKind`.
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

/// Extract a `ToolCallLocation` list from a tool's input JSON.
///
/// For file-oriented tools (Read, Edit, Write, …) we surface the file path so
/// that Zed can follow the agent while it works.
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

/// Build the `content` and `locations` for a `tool_call_update` notification.
///
/// Returns structured diff content for Edit/Write tools and a plain-text
/// content block (wrapped in a fenced code block) for Read.  All other tools
/// return empty vecs so the raw_output string remains the only result visible
/// in clients that don't speak ACP content blocks.
fn tool_result_content(
    tool_name: &str,
    input: Option<&serde_json::Value>,
    output: &str,
    status: ToolCallStatus,
) -> (Vec<ToolCallContent>, Vec<ToolCallLocation>) {
    match tool_name {
        "Edit" | "MultiEdit" => {
            let Some(inp) = input else {
                return (vec![], vec![]);
            };
            // Edit: { file_path, old_string, new_string }
            // MultiEdit: { file_path, edits: [{ old_string, new_string }] }
            let file_path = inp.get("file_path").and_then(|v| v.as_str());
            let Some(file_path) = file_path else {
                return (vec![], vec![]);
            };
            // Collect (old, new) pairs
            let pairs: Vec<(Option<&str>, &str)> = if tool_name == "MultiEdit" {
                inp.get("edits")
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
                let new = inp.get("new_string").and_then(|v| v.as_str());
                let old = inp.get("old_string").and_then(|v| v.as_str());
                if let Some(new) = new {
                    vec![(old, new)]
                } else {
                    vec![]
                }
            };
            if pairs.is_empty() {
                return (vec![], vec![]);
            }
            let content = pairs
                .into_iter()
                .map(|(old, new)| {
                    ToolCallContent::Diff(
                        Diff::new(file_path, new).old_text(old.map(str::to_string)),
                    )
                })
                .collect();
            let locations = vec![ToolCallLocation::new(file_path)];
            (content, locations)
        }
        "Write" | "NotebookEdit" => {
            let Some(inp) = input else {
                return (vec![], vec![]);
            };
            let file_path = inp.get("file_path").and_then(|v| v.as_str());
            let new_content = inp.get("content").and_then(|v| v.as_str());
            let (Some(file_path), Some(new_content)) = (file_path, new_content) else {
                return (vec![], vec![]);
            };
            let content = vec![ToolCallContent::Diff(Diff::new(file_path, new_content))];
            let locations = vec![ToolCallLocation::new(file_path)];
            (content, locations)
        }
        "Read" => {
            if output.trim().is_empty() || status == ToolCallStatus::Failed {
                return (vec![], vec![]);
            }
            let fenced = markdown_fence(output);
            let content = vec![ToolCallContent::from(ContentBlock::Text(TextContent::new(
                fenced,
            )))];
            (content, vec![])
        }
        _ => (vec![], vec![]),
    }
}

/// Wrap text in a fenced code block, extending the fence if the text itself
/// contains triple backticks (matching `markdownEscape` in the TS code).
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

/// Convert a `TodoWrite` `input` JSON value to ACP `PlanEntry` list.
///
/// Expected input shape: `{ "todos": [{ "content": "...", "status": "pending"|"in_progress"|"completed", "priority": "high"|"medium"|"low" }] }`
/// Returns `None` if the todos array is missing or empty.
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

/// Rewrite `/mcp:server:command args` → `/server:command (MCP) args`
/// to match Claude Code's internal slash command naming convention.
fn rewrite_mcp_slash_command(text: &str) -> String {
    // Match /mcp:server:command with optional trailing args
    if let Some(rest) = text.strip_prefix("/mcp:") {
        let (server_cmd, args) = rest
            .split_once(char::is_whitespace)
            .map(|(sc, a)| (sc, Some(a)))
            .unwrap_or((rest, None));
        if let Some((server, command)) = server_cmd.split_once(':') {
            let rewritten = match args {
                Some(a) => format!("/{server}:{command} (MCP) {a}"),
                None => format!("/{server}:{command} (MCP)"),
            };
            return rewritten;
        }
    }
    text.to_string()
}

/// Build `SessionConfigOption` list for the `ConfigOptionUpdate` sent after `EnterPlanMode`.
///
/// Mirrors `TrogonAcpAgent::build_config_options` from `trogon-acp`.  Duplicated here because
/// `acp-nats` cannot depend on the higher-level `trogon-acp` crate.  The `bypassPermissions`
/// mode is intentionally omitted — `EnterPlanMode` only ever sets mode to `"plan"`.
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

/// Logs a warning when the notification receiver has been dropped.
/// Extracted so that the unreachable-in-tests error path can be excluded from
/// coverage without suppressing the entire surrounding function.
#[cfg_attr(coverage, coverage(off))]
fn warn_notification_dropped() {
    warn!("notification receiver dropped; continuing prompt");
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── build_plan_mode_config_options ────────────────────────────────────────

    #[test]
    fn config_options_has_mode_and_model_entries() {
        let opts = build_plan_mode_config_options("plan", "claude-opus-4-6");
        assert_eq!(opts.len(), 2);
        assert_eq!(opts[0].id.0.as_ref(), "mode");
        assert_eq!(opts[1].id.0.as_ref(), "model");
    }

    #[test]
    fn config_options_mode_current_value_reflects_argument() {
        use agent_client_protocol::SessionConfigKind;
        let opts = build_plan_mode_config_options("plan", "claude-sonnet-4-6");
        let mode_opt = &opts[0];
        assert!(
            matches!(
                &mode_opt.kind,
                SessionConfigKind::Select(sel) if sel.current_value.0.as_ref() == "plan"
            ),
            "mode current_value should be 'plan'"
        );
    }

    #[test]
    fn config_options_model_current_value_reflects_argument() {
        use agent_client_protocol::SessionConfigKind;
        let opts = build_plan_mode_config_options("plan", "claude-haiku-4-5-20251001");
        let model_opt = &opts[1];
        assert!(
            matches!(
                &model_opt.kind,
                SessionConfigKind::Select(sel) if sel.current_value.0.as_ref() == "claude-haiku-4-5-20251001"
            ),
            "model current_value should be 'claude-haiku-4-5-20251001'"
        );
    }

    #[test]
    fn config_options_mode_does_not_include_bypass_permissions() {
        let opts = build_plan_mode_config_options("plan", "claude-opus-4-6");
        // bypassPermissions must NOT appear — EnterPlanMode never sets bypass
        let mode_opt = opts[0].clone();
        let has_bypass = serde_json::to_value(&mode_opt)
            .unwrap()
            .to_string()
            .contains("bypassPermissions");
        assert!(
            !has_bypass,
            "bypassPermissions should not appear in EnterPlanMode config options"
        );
    }

    #[test]
    fn config_options_mode_includes_standard_modes() {
        let opts = build_plan_mode_config_options("plan", "claude-opus-4-6");
        let json = serde_json::to_string(&opts[0]).unwrap();
        for expected in &["default", "acceptEdits", "plan", "dontAsk"] {
            assert!(json.contains(expected), "missing mode option: {expected}");
        }
    }

    #[test]
    fn config_options_model_includes_all_three_models() {
        let opts = build_plan_mode_config_options("plan", "claude-opus-4-6");
        let json = serde_json::to_string(&opts[1]).unwrap();
        for expected in &[
            "claude-opus-4-6",
            "claude-sonnet-4-6",
            "claude-haiku-4-5-20251001",
        ] {
            assert!(json.contains(expected), "missing model option: {expected}");
        }
    }

    // ── make_claude_code_meta ─────────────────────────────────────────────────

    #[test]
    fn make_claude_code_meta_has_tool_name() {
        let meta = make_claude_code_meta("EnterPlanMode");
        let cc = meta.get("claudeCode").unwrap().as_object().unwrap();
        assert_eq!(
            cc.get("toolName").unwrap().as_str().unwrap(),
            "EnterPlanMode"
        );
    }

    // ── rewrite_mcp_slash_command ─────────────────────────────────────────────

    #[test]
    fn rewrite_mcp_slash_command_with_args() {
        let out = rewrite_mcp_slash_command("/mcp:myserver:mytool some args");
        assert_eq!(out, "/myserver:mytool (MCP) some args");
    }

    #[test]
    fn rewrite_mcp_slash_command_without_args() {
        let out = rewrite_mcp_slash_command("/mcp:myserver:mytool");
        assert_eq!(out, "/myserver:mytool (MCP)");
    }

    #[test]
    fn rewrite_mcp_slash_command_non_mcp_passthrough() {
        let out = rewrite_mcp_slash_command("/compact");
        assert_eq!(out, "/compact");
    }

    #[test]
    fn rewrite_mcp_slash_command_plain_text_passthrough() {
        let out = rewrite_mcp_slash_command("hello world");
        assert_eq!(out, "hello world");
    }

    #[test]
    fn rewrite_mcp_slash_command_no_command_part_passthrough() {
        // /mcp:server has no ':command' segment — not a valid MCP slash command
        let out = rewrite_mcp_slash_command("/mcp:server_only");
        assert_eq!(out, "/mcp:server_only");
    }

    // ── format_uri_as_link ────────────────────────────────────────────────────

    #[test]
    fn format_uri_file_scheme_uses_last_segment() {
        let link = format_uri_as_link("file:///home/user/project/src/main.rs");
        assert_eq!(link, "[@main.rs](file:///home/user/project/src/main.rs)");
    }

    #[test]
    fn format_uri_zed_scheme_uses_last_segment() {
        let link = format_uri_as_link("zed://buffer/42");
        assert_eq!(link, "[@42](zed://buffer/42)");
    }

    #[test]
    fn format_uri_https_passes_through() {
        let uri = "https://example.com/some/path";
        let link = format_uri_as_link(uri);
        assert_eq!(link, uri);
    }

    #[test]
    fn format_uri_plain_text_passes_through() {
        let uri = "just some text";
        let link = format_uri_as_link(uri);
        assert_eq!(link, uri);
    }

    #[test]
    fn format_uri_file_with_trailing_slash_ignores_it() {
        // Trailing slash should not produce an empty display name.
        let link = format_uri_as_link("file:///home/user/project/");
        assert!(link.contains("[@project]"), "got: {link}");
    }

    // ── acp_blocks_to_user_content ────────────────────────────────────────────

    #[test]
    fn text_block_converts_to_text() {
        use agent_client_protocol::{ContentBlock, TextContent};
        let blocks = vec![ContentBlock::Text(TextContent::new("hello world"))];
        let result = acp_blocks_to_user_content(&blocks);
        assert_eq!(result.len(), 1);
        assert!(
            matches!(&result[0], crate::prompt_event::UserContentBlock::Text { text } if text == "hello world")
        );
    }

    #[test]
    fn resource_link_block_converts_to_resource_link() {
        use agent_client_protocol::{ContentBlock, ResourceLink};
        let blocks = vec![ContentBlock::ResourceLink(ResourceLink::new(
            "my-file.rs",
            "file:///src/my-file.rs",
        ))];
        let result = acp_blocks_to_user_content(&blocks);
        assert_eq!(result.len(), 1);
        assert!(
            matches!(&result[0], crate::prompt_event::UserContentBlock::ResourceLink { uri, name }
                if uri == "file:///src/my-file.rs" && name == "my-file.rs"),
        );
    }

    #[test]
    fn image_block_with_data_converts_to_image() {
        use agent_client_protocol::{ContentBlock, ImageContent};
        let blocks = vec![ContentBlock::Image(ImageContent::new(
            "base64data==",
            "image/png",
        ))];
        let result = acp_blocks_to_user_content(&blocks);
        assert_eq!(result.len(), 1);
        assert!(
            matches!(&result[0], crate::prompt_event::UserContentBlock::Image { data, mime_type }
                if data == "base64data==" && mime_type == "image/png"),
        );
    }

    #[test]
    fn image_block_with_https_uri_converts_to_image_url() {
        use agent_client_protocol::{ContentBlock, ImageContent};
        let blocks = vec![ContentBlock::Image(
            ImageContent::new("", "image/jpeg").uri("https://example.com/photo.jpg".to_string()),
        )];
        let result = acp_blocks_to_user_content(&blocks);
        assert_eq!(result.len(), 1);
        assert!(
            matches!(&result[0], crate::prompt_event::UserContentBlock::ImageUrl { url }
                if url == "https://example.com/photo.jpg"),
        );
    }

    #[test]
    fn image_block_with_non_http_uri_becomes_text_link() {
        use agent_client_protocol::{ContentBlock, ImageContent};
        let blocks = vec![ContentBlock::Image(
            ImageContent::new("", "image/jpeg").uri("data:image/jpeg;base64,abc".to_string()),
        )];
        let result = acp_blocks_to_user_content(&blocks);
        assert_eq!(result.len(), 1);
        assert!(
            matches!(&result[0], crate::prompt_event::UserContentBlock::Text { text }
                if text.starts_with("![image](")),
        );
    }

    #[test]
    fn image_block_with_empty_data_and_no_uri_is_skipped() {
        use agent_client_protocol::{ContentBlock, ImageContent};
        // No base64 data AND no URI — should produce no output block
        let blocks = vec![ContentBlock::Image(ImageContent::new("", "image/png"))];
        let result = acp_blocks_to_user_content(&blocks);
        assert!(
            result.is_empty(),
            "image with no data and no URI should be skipped"
        );
    }

    #[test]
    fn embedded_resource_text_produces_link_plus_context_at_end() {
        use agent_client_protocol::{
            ContentBlock, EmbeddedResource, EmbeddedResourceResource, TextContent,
            TextResourceContents,
        };
        let blocks = vec![
            ContentBlock::Text(TextContent::new("before")),
            ContentBlock::Resource(EmbeddedResource::new(
                EmbeddedResourceResource::TextResourceContents(TextResourceContents::new(
                    "fn main() {}",
                    "file:///src/main.rs",
                )),
            )),
            ContentBlock::Text(TextContent::new("after")),
        ];
        let result = acp_blocks_to_user_content(&blocks);
        // Expect: "before", inline file link, "after", context block (at end)
        assert_eq!(result.len(), 4, "expected 4 blocks, got: {result:?}");
        assert!(
            matches!(&result[0], crate::prompt_event::UserContentBlock::Text { text } if text == "before")
        );
        // inline reference link for the resource
        assert!(
            matches!(&result[1], crate::prompt_event::UserContentBlock::Text { text } if text.contains("file:///src/main.rs"))
        );
        assert!(
            matches!(&result[2], crate::prompt_event::UserContentBlock::Text { text } if text == "after")
        );
        // context block appended at end
        assert!(
            matches!(&result[3], crate::prompt_event::UserContentBlock::Context { uri, text }
                if uri == "file:///src/main.rs" && text == "fn main() {}"),
        );
    }

    #[test]
    fn embedded_resource_blob_is_skipped() {
        use agent_client_protocol::{
            BlobResourceContents, ContentBlock, EmbeddedResource, EmbeddedResourceResource,
        };
        let blocks = vec![ContentBlock::Resource(EmbeddedResource::new(
            EmbeddedResourceResource::BlobResourceContents(BlobResourceContents::new(
                "binarydata==",
                "file:///image.bin",
            )),
        ))];
        let result = acp_blocks_to_user_content(&blocks);
        assert!(result.is_empty(), "blob resources must be skipped");
    }

    #[test]
    fn audio_block_is_skipped() {
        use agent_client_protocol::{AudioContent, ContentBlock};
        let blocks = vec![ContentBlock::Audio(AudioContent::new(
            "audiodata==",
            "audio/mp3",
        ))];
        let result = acp_blocks_to_user_content(&blocks);
        assert!(result.is_empty(), "audio blocks must be skipped");
    }

    #[test]
    fn context_blocks_always_appended_after_main_content() {
        use agent_client_protocol::{
            ContentBlock, EmbeddedResource, EmbeddedResourceResource, TextContent,
            TextResourceContents,
        };
        // Two embedded text resources followed by a text block
        let blocks = vec![
            ContentBlock::Resource(EmbeddedResource::new(
                EmbeddedResourceResource::TextResourceContents(TextResourceContents::new(
                    "content A",
                    "file:///a.rs",
                )),
            )),
            ContentBlock::Resource(EmbeddedResource::new(
                EmbeddedResourceResource::TextResourceContents(TextResourceContents::new(
                    "content B",
                    "file:///b.rs",
                )),
            )),
            ContentBlock::Text(TextContent::new("user message")),
        ];
        let result = acp_blocks_to_user_content(&blocks);
        // 2 inline links + 1 text + 2 context blocks = 5
        assert_eq!(result.len(), 5);
        // Last two must be Context blocks
        assert!(matches!(
            &result[3],
            crate::prompt_event::UserContentBlock::Context { .. }
        ));
        assert!(matches!(
            &result[4],
            crate::prompt_event::UserContentBlock::Context { .. }
        ));
    }

    // ── todo_write_to_plan_entries ────────────────────────────────────────────

    #[test]
    fn todo_write_to_plan_entries_maps_statuses() {
        let input = serde_json::json!({
            "todos": [
                { "content": "Do A", "status": "pending",     "priority": "high" },
                { "content": "Do B", "status": "in_progress", "priority": "medium" },
                { "content": "Do C", "status": "completed",   "priority": "low" },
            ]
        });
        let entries = todo_write_to_plan_entries(&input).unwrap();
        assert_eq!(entries.len(), 3);
        assert!(matches!(entries[0].status, PlanEntryStatus::Pending));
        assert!(matches!(entries[1].status, PlanEntryStatus::InProgress));
        assert!(matches!(entries[2].status, PlanEntryStatus::Completed));
    }

    #[test]
    fn todo_write_to_plan_entries_maps_priorities() {
        let input = serde_json::json!({
            "todos": [
                { "content": "A", "status": "pending", "priority": "high" },
                { "content": "B", "status": "pending", "priority": "medium" },
                { "content": "C", "status": "pending", "priority": "low" },
            ]
        });
        let entries = todo_write_to_plan_entries(&input).unwrap();
        assert!(matches!(entries[0].priority, PlanEntryPriority::High));
        assert!(matches!(entries[1].priority, PlanEntryPriority::Medium));
        assert!(matches!(entries[2].priority, PlanEntryPriority::Low));
    }

    #[test]
    fn todo_write_to_plan_entries_returns_none_for_empty_todos() {
        let input = serde_json::json!({ "todos": [] });
        assert!(todo_write_to_plan_entries(&input).is_none());
    }

    #[test]
    fn todo_write_to_plan_entries_returns_none_without_todos_key() {
        let input = serde_json::json!({});
        assert!(todo_write_to_plan_entries(&input).is_none());
    }

    // ── make_meta_with_terminal_exit ──────────────────────────────────────────

    #[test]
    fn make_meta_with_terminal_exit_signal_some_inserts_string() {
        let meta = make_meta_with_terminal_exit("Bash", "t-1", Some(0), Some("SIGTERM"));
        let exit = meta["terminal_exit"].as_object().unwrap();
        assert_eq!(
            exit["signal"].as_str().unwrap(),
            "SIGTERM",
            "signal Some should produce a JSON string"
        );
    }

    // ── tool_kind_for ─────────────────────────────────────────────────────────

    #[test]
    fn tool_kind_for_search_tools() {
        assert!(matches!(tool_kind_for("Glob"), ToolKind::Search));
        assert!(matches!(tool_kind_for("Grep"), ToolKind::Search));
    }

    #[test]
    fn tool_kind_for_fetch_tools() {
        assert!(matches!(tool_kind_for("WebSearch"), ToolKind::Fetch));
        assert!(matches!(tool_kind_for("WebFetch"), ToolKind::Fetch));
    }

    #[test]
    fn tool_kind_for_think() {
        assert!(matches!(tool_kind_for("Think"), ToolKind::Think));
    }

    #[test]
    fn tool_kind_for_switch_mode() {
        assert!(matches!(
            tool_kind_for("ExitPlanMode"),
            ToolKind::SwitchMode
        ));
        assert!(matches!(
            tool_kind_for("EnterPlanMode"),
            ToolKind::SwitchMode
        ));
    }

    #[test]
    fn tool_kind_for_unknown_returns_other() {
        assert!(matches!(tool_kind_for("UnknownTool"), ToolKind::Other));
    }

    // ── tool_locations_from_input ─────────────────────────────────────────────

    #[test]
    fn tool_locations_from_input_returns_empty_when_path_key_absent() {
        let input = serde_json::json!({"other": "value"});
        let locs = tool_locations_from_input("Read", &input);
        assert!(
            locs.is_empty(),
            "should return empty when path key is absent"
        );
    }

    // ── tool_result_content ───────────────────────────────────────────────────

    #[test]
    fn tool_result_content_edit_no_input_returns_empty() {
        let (c, l) = tool_result_content("Edit", None, "", ToolCallStatus::Completed);
        assert!(c.is_empty() && l.is_empty());
    }

    #[test]
    fn tool_result_content_edit_no_file_path_returns_empty() {
        let input = serde_json::json!({"new_string": "x"});
        let (c, l) = tool_result_content("Edit", Some(&input), "", ToolCallStatus::Completed);
        assert!(c.is_empty() && l.is_empty());
    }

    #[test]
    fn tool_result_content_edit_no_new_string_returns_empty() {
        let input = serde_json::json!({"file_path": "/f.rs"});
        let (c, l) = tool_result_content("Edit", Some(&input), "", ToolCallStatus::Completed);
        assert!(c.is_empty() && l.is_empty());
    }

    #[test]
    fn tool_result_content_multi_edit_produces_diff_per_edit() {
        let input = serde_json::json!({
            "file_path": "/f.rs",
            "edits": [
                {"old_string": "old1", "new_string": "new1"},
                {"new_string": "new2"}
            ]
        });
        let (c, l) = tool_result_content("MultiEdit", Some(&input), "", ToolCallStatus::Completed);
        assert_eq!(c.len(), 2, "two edits → two diff blocks");
        assert_eq!(l.len(), 1, "one file location");
    }

    #[test]
    fn tool_result_content_multi_edit_empty_edits_returns_empty() {
        let input = serde_json::json!({"file_path": "/f.rs", "edits": []});
        let (c, l) = tool_result_content("MultiEdit", Some(&input), "", ToolCallStatus::Completed);
        assert!(c.is_empty() && l.is_empty(), "no edits → empty result");
    }

    #[test]
    fn tool_result_content_write_produces_diff() {
        let input = serde_json::json!({"file_path": "/w.rs", "content": "fn main() {}"});
        let (c, l) = tool_result_content("Write", Some(&input), "", ToolCallStatus::Completed);
        assert_eq!(c.len(), 1);
        assert_eq!(l.len(), 1);
    }

    #[test]
    fn tool_result_content_write_no_input_returns_empty() {
        let (c, l) = tool_result_content("Write", None, "", ToolCallStatus::Completed);
        assert!(c.is_empty() && l.is_empty());
    }

    #[test]
    fn tool_result_content_notebook_edit_no_content_returns_empty() {
        let input = serde_json::json!({"file_path": "/nb.ipynb"});
        let (c, l) =
            tool_result_content("NotebookEdit", Some(&input), "", ToolCallStatus::Completed);
        assert!(c.is_empty() && l.is_empty());
    }

    #[test]
    fn tool_result_content_read_empty_output_returns_empty() {
        let (c, l) = tool_result_content("Read", None, "  ", ToolCallStatus::Completed);
        assert!(
            c.is_empty() && l.is_empty(),
            "whitespace-only output → empty"
        );
    }

    #[test]
    fn tool_result_content_read_failed_status_returns_empty() {
        let (c, l) = tool_result_content("Read", None, "some output", ToolCallStatus::Failed);
        assert!(c.is_empty() && l.is_empty(), "failed status → empty");
    }

    // ── markdown_fence ────────────────────────────────────────────────────────

    #[test]
    fn markdown_fence_extends_fence_for_backtick_content() {
        let output = "```\nsome code\n```";
        let fenced = markdown_fence(output);
        assert!(
            fenced.starts_with("````"),
            "fence must be extended when content has ```, got: {fenced}"
        );
    }
}

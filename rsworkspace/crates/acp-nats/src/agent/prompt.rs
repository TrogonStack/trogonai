use agent_client_protocol::{
    ContentBlock, ContentChunk, EmbeddedResourceResource, Error, ErrorCode, Plan, PlanEntry,
    PlanEntryPriority, PlanEntryStatus, PromptRequest, PromptResponse, SessionNotification,
    SessionUpdate, StopReason, TextContent, ToolCall, ToolCallStatus, ToolCallUpdate,
    ToolCallUpdateFields, Usage, UsageUpdate,
};
use crate::prompt_event::UserContentBlock;
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
    let session_id = AcpSessionId::try_from(&args.session_id).map_err(|_| {
        Error::new(ErrorCode::InvalidParams.into(), "invalid session id")
    })?;

    // 2. Convert ACP content blocks to rich UserContentBlocks for the runner
    let content = acp_blocks_to_user_content(&args.prompt);

    // Plain-text fallback (used for title, backward compat)
    let user_message = args
        .prompt
        .iter()
        .filter_map(|block| {
            if let ContentBlock::Text(t) = block { Some(t.text.as_str()) } else { None }
        })
        .collect::<Vec<_>>()
        .join("\n");

    // 3. Generate a unique request ID to correlate the response event stream
    let req_id = uuid::Uuid::new_v4().to_string();

    // 4. Subscribe to events BEFORE publishing the prompt — prevents losing the
    //    first event in case the runner is already running and responds instantly
    let events_subject =
        agent::prompt_events(bridge.config.acp_prefix(), session_id.as_ref(), &req_id);

    let mut subscriber = bridge.nats.subscribe(events_subject).await.map_err(|e| {
        Error::new(ErrorCode::InternalError.into(), format!("subscribe: {e}"))
    })?;

    let session_cancelled_subject =
        agent::session_cancelled(bridge.config.acp_prefix(), session_id.as_ref());
    let mut cancel_notify = bridge.nats.subscribe(session_cancelled_subject).await.map_err(|e| {
        Error::new(ErrorCode::InternalError.into(), format!("subscribe cancelled: {e}"))
    })?;

    // 5. Build and publish the prompt payload via NATS Core
    let payload = PromptPayload {
        req_id,
        session_id: session_id.to_string(),
        content,
        user_message,
    };
    let payload_bytes =
        serde_json::to_vec(&payload).map_err(|e| Error::new(ErrorCode::InternalError.into(), e.to_string()))?;

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
    // ids of TodoWrite calls — emitted as Plan updates, no ToolCallUpdate on finish
    let mut todo_write_ids: std::collections::HashSet<String> = std::collections::HashSet::new();

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
                let total = accumulated_input + accumulated_output
                    + accumulated_cache_creation + accumulated_cache_read;
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

                // TodoWrite → emit Plan update instead of a tool_call notification
                if name == "TodoWrite" {
                    if let Some(entries) = todo_write_to_plan_entries(&input) {
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
                }

                let tool_call = ToolCall::new(id, name.clone())
                    .status(ToolCallStatus::InProgress)
                    .raw_input(input)
                    .meta(make_claude_code_meta(&name));
                let notification = SessionNotification::new(
                    args.session_id.clone(),
                    SessionUpdate::ToolCall(tool_call),
                );
                if bridge.notification_sender.send(notification).await.is_err() {
                    warn!("notification receiver dropped; continuing prompt");
                }
            }
            PromptEvent::ToolCallFinished { id, output, exit_code, signal } => {
                // TodoWrite finished — Plan was already sent, skip the tool_call_update
                if todo_write_ids.contains(&id) {
                    continue;
                }
                let status = if exit_code.map(|c| c != 0).unwrap_or(false) || signal.is_some() {
                    ToolCallStatus::Failed
                } else {
                    ToolCallStatus::Completed
                };
                let fields = ToolCallUpdateFields::new()
                    .status(status)
                    .raw_output(serde_json::Value::String(output));
                let update = ToolCallUpdate::new(id.clone(), fields);
                let update = if let Some(name) = tool_name_cache.get(&id) {
                    update.meta(make_claude_code_meta(name))
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
            PromptEvent::SystemStatus { message } => {
                tracing::info!(message = %message, "agent system status");
            }
            PromptEvent::UsageUpdate { input_tokens, output_tokens, cache_creation_tokens, cache_read_tokens, context_window } => {
                accumulated_input += input_tokens as u64;
                accumulated_output += output_tokens as u64;
                accumulated_cache_creation += cache_creation_tokens as u64;
                accumulated_cache_read += cache_read_tokens as u64;
                let used = accumulated_input + accumulated_output + accumulated_cache_read + accumulated_cache_creation;
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

/// Convert ACP `ContentBlock`s to `UserContentBlock`s for transport over NATS.
///
/// Follows the same logic as `promptToClaude()` in the TypeScript reference:
/// - Text → plain text
/// - ResourceLink → `[@name](uri)` formatted link
/// - Resource (text) → inline [@name](uri) link + `<context ref="uri">\n{text}\n</context>` appended at the end
/// - Resource (blob) → skipped
/// - Image (base64) → image block
/// - Image (http/https url) → native URL image block; other URL schemes → `![image](url)` text link

/// Format a URI as an inline reference link, matching TS `formatUriAsLink`.
/// For file:// and zed:// URIs, extracts the last path segment as the display name.
/// For other URIs, uses the full URI as-is.
fn format_uri_as_link(uri: &str) -> String {
    if uri.starts_with("file://") || uri.starts_with("zed://") {
        let name = uri.trim_end_matches('/')
            .rsplit('/')
            .next()
            .filter(|s| !s.is_empty())
            .unwrap_or(uri);
        format!("[@{name}]({uri})")
    } else {
        uri.to_string()
    }
}

fn acp_blocks_to_user_content(blocks: &[ContentBlock]) -> Vec<UserContentBlock> {
    let mut content: Vec<UserContentBlock> = Vec::new();
    let mut context_parts: Vec<UserContentBlock> = Vec::new();

    for block in blocks {
        match block {
            ContentBlock::Text(t) => {
                let text = rewrite_mcp_slash_command(&t.text);
                content.push(UserContentBlock::Text { text });
            }
            ContentBlock::Image(img) => {
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
            }
            ContentBlock::ResourceLink(r) => {
                content.push(UserContentBlock::ResourceLink {
                    uri: r.uri.clone(),
                    name: r.name.clone(),
                });
            }
            ContentBlock::Resource(r) => {
                match &r.resource {
                    EmbeddedResourceResource::TextResourceContents(t) => {
                        // Inline reference link (position marker in the message body)
                        content.push(UserContentBlock::Text {
                            text: format_uri_as_link(&t.uri),
                        });
                        context_parts.push(UserContentBlock::Context {
                            uri: t.uri.clone(),
                            text: t.text.clone(),
                        });
                    }
                    EmbeddedResourceResource::BlobResourceContents(_) => {
                        // Binary blobs can't be sent to text/image models — skip
                    }
                    _ => {
                        // Future resource types — skip
                    }
                }
            }
            ContentBlock::Audio(_) => {
                // Audio not supported — skip
            }
            _ => {
                // Future content block types — skip
            }
        }
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
    meta.insert("claudeCode".to_string(), serde_json::Value::Object(claude_code));
    meta
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
    if entries.is_empty() { None } else { Some(entries) }
}

/// Rewrite `/mcp:server:command args` → `/server:command (MCP) args`
/// to match Claude Code's internal slash command naming convention.
fn rewrite_mcp_slash_command(text: &str) -> String {
    // Match /mcp:server:command with optional trailing args
    if let Some(rest) = text.strip_prefix("/mcp:") {
        let (server_cmd, args) = rest.split_once(char::is_whitespace)
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

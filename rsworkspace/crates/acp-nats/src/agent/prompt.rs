use agent_client_protocol::{
    Error, ErrorCode, PromptRequest, PromptResponse, SessionNotification, StopReason,
};
use bytes::Bytes;
use futures::StreamExt;
use tokio::time::timeout;
use tracing::{instrument, warn};
use trogon_std::JsonSerialize;

use crate::agent::Bridge;
use crate::nats::{FlushClient, PublishClient, RequestClient, SubscribeClient, agent};
use crate::session_id::AcpSessionId;

pub const REQ_ID_HEADER: &str = "X-Req-Id";

#[instrument(
    name = "acp.session.prompt",
    skip(bridge, args, serializer),
    fields(session_id = %args.session_id)
)]
pub async fn handle<N, C, S>(
    bridge: &Bridge<N, C>,
    args: PromptRequest,
    serializer: &S,
) -> agent_client_protocol::Result<PromptResponse>
where
    N: RequestClient + PublishClient + SubscribeClient + FlushClient,
    C: trogon_std::time::GetElapsed,
    S: JsonSerialize,
{
    let start = bridge.clock.now();

    let session_id = AcpSessionId::try_from(&args.session_id).map_err(|_| {
        bridge.metrics.record_error("prompt", "invalid_session_id");
        Error::new(ErrorCode::InvalidParams.into(), "Invalid session ID")
    })?;

    let req_id = uuid::Uuid::new_v4().to_string();
    let sid = session_id.as_ref();
    let prefix = bridge.config.acp_prefix();

    // Subscribe BEFORE publishing — prevents losing the first event if the runner responds instantly.
    let mut notifications_sub = bridge
        .nats
        .subscribe(agent::session_update(prefix, sid, &req_id))
        .await
        .map_err(|e| Error::new(ErrorCode::InternalError.into(), format!("subscribe: {e}")))?;

    let mut response_sub = bridge
        .nats
        .subscribe(agent::ext_session_prompt_response(prefix, sid, &req_id))
        .await
        .map_err(|e| Error::new(ErrorCode::InternalError.into(), format!("subscribe: {e}")))?;

    let mut cancel_sub = bridge
        .nats
        .subscribe(agent::session_cancelled(prefix, sid))
        .await
        .map_err(|e| {
            Error::new(
                ErrorCode::InternalError.into(),
                format!("subscribe cancelled: {e}"),
            )
        })?;

    let payload_bytes = serializer
        .to_vec(&args)
        .map_err(|e| Error::new(ErrorCode::InternalError.into(), format!("serialize: {e}")))?;

    let mut headers = async_nats::HeaderMap::new();
    headers.insert(REQ_ID_HEADER, req_id.as_str());

    let prompt_subject = agent::session_prompt(prefix, sid);
    bridge
        .nats
        .publish_with_headers(prompt_subject, headers, Bytes::from(payload_bytes))
        .await
        .map_err(|e| Error::new(ErrorCode::InternalError.into(), format!("publish: {e}")))?;

    bridge
        .nats
        .flush()
        .await
        .map_err(|e| Error::new(ErrorCode::InternalError.into(), format!("flush: {e}")))?;

    let op_timeout = bridge.config.prompt_timeout();

    let result = loop {
        tokio::select! {
            notif = notifications_sub.next() => {
                let Some(msg) = notif else {
                    bridge.metrics.record_error("prompt", "notification_stream_closed");
                    break Err(Error::new(
                        ErrorCode::InternalError.into(),
                        "notification stream closed unexpectedly",
                    ));
                };
                let notification: SessionNotification = match serde_json::from_slice(&msg.payload) {
                    Ok(n) => n,
                    Err(e) => {
                        warn!(error = %e, "bad notification payload; skipping");
                        continue;
                    }
                };
                if bridge.notification_sender.send(notification).await.is_err() {
                    warn!("notification receiver dropped; continuing prompt");
                }
            }
            resp = timeout(op_timeout, response_sub.next()) => {
                match resp {
                    Ok(Some(msg)) => {
                        match serde_json::from_slice::<PromptResponse>(&msg.payload) {
                            Ok(response) => break Ok(response),
                            Err(e) => {
                                bridge.metrics.record_error("prompt", "bad_response_payload");
                                break Err(Error::new(
                                    ErrorCode::InternalError.into(),
                                    format!("bad response payload: {e}"),
                                ));
                            }
                        }
                    }
                    Ok(None) => {
                        bridge.metrics.record_error("prompt", "response_stream_closed");
                        break Err(Error::new(
                            ErrorCode::InternalError.into(),
                            "response stream closed unexpectedly",
                        ));
                    }
                    Err(_elapsed) => {
                        bridge.metrics.record_error("prompt", "prompt_timeout");
                        break Err(Error::new(
                            ErrorCode::InternalError.into(),
                            "prompt timed out waiting for runner",
                        ));
                    }
                }
            }
            _ = cancel_sub.next() => {
                break Ok(PromptResponse::new(StopReason::Cancelled));
            }
        }
    };

    bridge.metrics.record_request(
        "prompt",
        bridge.clock.elapsed(start).as_secs_f64(),
        result.is_ok(),
    );

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
    use crate::config::Config;
    use trogon_nats::AdvancedMockNatsClient;

    fn make_nats_msg(payload: &[u8]) -> async_nats::Message {
        async_nats::Message {
            subject: "test".into(),
            reply: None,
            payload: bytes::Bytes::from(payload.to_vec()),
            headers: None,
            status: None,
            description: None,
            length: payload.len(),
        }
    }

    fn mock_bridge() -> (
        AdvancedMockNatsClient,
        Bridge<AdvancedMockNatsClient, trogon_std::time::SystemClock>,
    ) {
        let mock = AdvancedMockNatsClient::new();
        let (notification_tx, _notification_rx) =
            tokio::sync::mpsc::channel::<SessionNotification>(64);
        let bridge = Bridge::new(
            mock.clone(),
            trogon_std::time::SystemClock,
            &opentelemetry::global::meter("prompt-test"),
            Config::for_test("acp"),
            notification_tx,
        );
        (mock, bridge)
    }

    #[tokio::test]
    async fn prompt_returns_error_when_subscribe_fails() {
        let (_mock, bridge) = mock_bridge();
        let result = handle(
            &bridge,
            PromptRequest::new("s1", vec![]),
            &trogon_std::StdJsonSerialize,
        )
        .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn prompt_rejects_invalid_session_id() {
        let (_mock, bridge) = mock_bridge();
        let err = handle(
            &bridge,
            PromptRequest::new("invalid.session.id", vec![]),
            &trogon_std::StdJsonSerialize,
        )
        .await
        .unwrap_err();
        assert_eq!(err.code, ErrorCode::InvalidParams);
    }

    #[tokio::test]
    async fn prompt_returns_done_response_from_runner() {
        let (mock, bridge) = mock_bridge();

        let _notif_tx = mock.inject_messages();
        let resp_tx = mock.inject_messages();
        let _cancel_tx = mock.inject_messages();

        let response = PromptResponse::new(StopReason::EndTurn);
        resp_tx
            .unbounded_send(make_nats_msg(&serde_json::to_vec(&response).unwrap()))
            .unwrap();

        let result = handle(
            &bridge,
            PromptRequest::new("s1", vec![]),
            &trogon_std::StdJsonSerialize,
        )
        .await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap().stop_reason, StopReason::EndTurn);
    }

    #[tokio::test]
    async fn prompt_returns_cancelled_on_cancel_signal() {
        let (mock, bridge) = mock_bridge();

        let _notif_tx = mock.inject_messages();
        let _resp_tx = mock.inject_messages();
        let cancel_tx = mock.inject_messages();

        cancel_tx.unbounded_send(make_nats_msg(b"")).unwrap();

        let result = handle(
            &bridge,
            PromptRequest::new("s1", vec![]),
            &trogon_std::StdJsonSerialize,
        )
        .await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap().stop_reason, StopReason::Cancelled);
    }

    #[tokio::test]
    async fn prompt_returns_error_on_bad_response_payload() {
        let (mock, bridge) = mock_bridge();

        let _notif_tx = mock.inject_messages();
        let resp_tx = mock.inject_messages();
        let _cancel_tx = mock.inject_messages();

        resp_tx.unbounded_send(make_nats_msg(b"not json")).unwrap();

        let result = handle(
            &bridge,
            PromptRequest::new("s1", vec![]),
            &trogon_std::StdJsonSerialize,
        )
        .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn prompt_returns_error_when_response_stream_closes() {
        let (mock, bridge) = mock_bridge();

        let _notif_tx = mock.inject_messages();
        let resp_tx = mock.inject_messages();
        let _cancel_tx = mock.inject_messages();

        drop(resp_tx);

        let result = handle(
            &bridge,
            PromptRequest::new("s1", vec![]),
            &trogon_std::StdJsonSerialize,
        )
        .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn prompt_returns_error_when_second_subscribe_fails() {
        let (mock, bridge) = mock_bridge();
        let _notif_tx = mock.inject_messages();
        // No second stream — second subscribe fails
        let result = handle(
            &bridge,
            PromptRequest::new("s1", vec![]),
            &trogon_std::StdJsonSerialize,
        )
        .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn prompt_returns_error_when_third_subscribe_fails() {
        let (mock, bridge) = mock_bridge();
        let _notif_tx = mock.inject_messages();
        let _resp_tx = mock.inject_messages();
        // No third stream — third subscribe fails
        let result = handle(
            &bridge,
            PromptRequest::new("s1", vec![]),
            &trogon_std::StdJsonSerialize,
        )
        .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn prompt_returns_error_when_serialize_fails() {
        let (mock, bridge) = mock_bridge();

        let _notif_tx = mock.inject_messages();
        let _resp_tx = mock.inject_messages();
        let _cancel_tx = mock.inject_messages();

        let result = handle(
            &bridge,
            PromptRequest::new("s1", vec![]),
            &trogon_std::FailNextSerialize::new(1),
        )
        .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn prompt_returns_error_when_publish_fails() {
        let (mock, bridge) = mock_bridge();

        let _notif_tx = mock.inject_messages();
        let _resp_tx = mock.inject_messages();
        let _cancel_tx = mock.inject_messages();
        mock.fail_next_publish();

        let result = handle(
            &bridge,
            PromptRequest::new("s1", vec![]),
            &trogon_std::StdJsonSerialize,
        )
        .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn prompt_returns_error_when_flush_fails() {
        let (mock, bridge) = mock_bridge();

        let _notif_tx = mock.inject_messages();
        let _resp_tx = mock.inject_messages();
        let _cancel_tx = mock.inject_messages();
        mock.fail_next_flush();

        let result = handle(
            &bridge,
            PromptRequest::new("s1", vec![]),
            &trogon_std::StdJsonSerialize,
        )
        .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn prompt_publishes_to_correct_subject() {
        let (mock, bridge) = mock_bridge();

        let _notif_tx = mock.inject_messages();
        let resp_tx = mock.inject_messages();
        let _cancel_tx = mock.inject_messages();

        let response = PromptResponse::new(StopReason::EndTurn);
        resp_tx
            .unbounded_send(make_nats_msg(&serde_json::to_vec(&response).unwrap()))
            .unwrap();

        let _ = handle(
            &bridge,
            PromptRequest::new("s1", vec![]),
            &trogon_std::StdJsonSerialize,
        )
        .await;

        let subjects = mock.published_messages();
        assert!(
            subjects.iter().any(|s| s == "acp.s1.agent.session.prompt"),
            "expected publish to acp.s1.agent.session.prompt, got: {:?}",
            subjects
        );
    }

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
        // The conditional avoids adding an extra '\n' when text already ends with one,
        // so the result must have exactly one blank line before the fence (not two).
        assert!(
            !fenced.contains("\n\n\n```"),
            "should not triple-newline when text ends with newline, got: {fenced:?}"
        );
        assert!(
            fenced.ends_with("\n```"),
            "must end with closing fence, got: {fenced:?}"
        );
    }

    // ── tool_kind_for (remaining arms) ────────────────────────────────────────

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

    // ── tool_locations_from_input (success paths) ─────────────────────────────

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

    // ── make_meta_with_terminal_info ──────────────────────────────────────────

    #[test]
    fn make_meta_with_terminal_info_contains_terminal_id() {
        let meta = make_meta_with_terminal_info("Bash", "term-42");
        let info = meta["terminal_info"].as_object().unwrap();
        assert_eq!(info["terminal_id"].as_str().unwrap(), "term-42");
        // claudeCode.toolName must also be present
        let cc = meta["claudeCode"].as_object().unwrap();
        assert_eq!(cc["toolName"].as_str().unwrap(), "Bash");
    }

    // ── make_meta_with_terminal_output ────────────────────────────────────────

    #[test]
    fn make_meta_with_terminal_output_contains_id_and_data() {
        let meta = make_meta_with_terminal_output("Bash", "term-7", "stdout line\n");
        let out = meta["terminal_output"].as_object().unwrap();
        assert_eq!(out["terminal_id"].as_str().unwrap(), "term-7");
        assert_eq!(out["data"].as_str().unwrap(), "stdout line\n");
    }

    // ── make_meta_with_terminal_exit (None signal) ────────────────────────────

    #[test]
    fn make_meta_with_terminal_exit_signal_none_inserts_null() {
        let meta = make_meta_with_terminal_exit("Bash", "t-1", Some(0), None);
        let exit = meta["terminal_exit"].as_object().unwrap();
        assert_eq!(
            exit["signal"],
            serde_json::Value::Null,
            "signal None should produce JSON null"
        );
    }
}

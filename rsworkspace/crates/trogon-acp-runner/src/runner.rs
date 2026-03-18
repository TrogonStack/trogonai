use std::sync::Arc;

use acp_nats::nats::agent as subjects;
use acp_nats::prompt_event::{PromptEvent, PromptPayload, UserContentBlock};
use async_nats::jetstream;
use bytes::Bytes;
use futures_util::StreamExt;
use tokio::sync::{RwLock, mpsc};
use tracing::{error, info, warn};

/// Gateway credentials that override the default proxy/token when set.
/// Populated by `authenticate()` in the ACP agent and shared with the Runner.
#[derive(Debug, Clone)]
pub struct GatewayConfig {
    /// Full base URL for the Anthropic messages endpoint (e.g. `https://gateway.example.com/v1`).
    pub base_url: String,
    /// Auth token for the gateway.
    pub token: String,
    /// Additional HTTP headers to forward to the gateway.
    pub extra_headers: Vec<(String, String)>,
}

use trogon_agent::agent_loop::{AgentEvent, AgentLoop, ContentBlock, ImageSource, Message};
use trogon_agent::tools::{ToolDef, all_tool_defs};

use crate::permission::{ChannelPermissionChecker, PermissionTx};
use crate::session_store::{SessionStore, StoredMcpServer};

/// Returns the context window token limit for a given model ID.
fn context_window_tokens(model: &str) -> u64 {
    if model.contains("opus") || model.contains("sonnet") || model.contains("haiku") {
        200_000
    } else {
        200_000 // safe default
    }
}

/// Subscribes to `{prefix}.*.agent.prompt` via NATS Core, runs the agentic loop
/// for each incoming prompt (with streaming events and cancel support), and publishes
/// `PromptEvent` messages back to the Bridge.
pub struct Runner {
    nats: async_nats::Client,
    store: SessionStore,
    agent: Arc<AgentLoop>,
    prefix: String,
    /// Optional in-process channel to forward permission requests to the ACP connection.
    /// `None` means all tools are auto-allowed (no gate).
    permission_tx: Option<PermissionTx>,
    /// Optional gateway config — when set, overrides proxy_url/anthropic_token on the agent.
    gateway_config: Arc<RwLock<Option<GatewayConfig>>>,
}

impl Runner {
    pub async fn new(
        nats: async_nats::Client,
        js: &jetstream::Context,
        agent: AgentLoop,
        prefix: impl Into<String>,
        permission_tx: Option<PermissionTx>,
        gateway_config: Arc<RwLock<Option<GatewayConfig>>>,
    ) -> anyhow::Result<Self> {
        let store = SessionStore::open(js).await?;
        Ok(Self {
            nats,
            store,
            agent: Arc::new(agent),
            prefix: prefix.into(),
            permission_tx,
            gateway_config,
        })
    }

    /// Run the prompt subscriber loop — returns when the NATS connection closes.
    pub async fn run(self) {
        let wildcard = subjects::prompt_wildcard(&self.prefix);
        let mut sub = match self.nats.subscribe(wildcard.clone()).await {
            Ok(s) => s,
            Err(e) => {
                error!(subject = %wildcard, error = %e, "runner: failed to subscribe");
                return;
            }
        };

        info!(subject = %wildcard, "runner: listening for prompts");

        while let Some(msg) = sub.next().await {
            let payload: PromptPayload = match serde_json::from_slice(&msg.payload) {
                Ok(p) => p,
                Err(e) => {
                    warn!(error = %e, "runner: bad prompt payload — skipping");
                    continue;
                }
            };

            let events_subject = subjects::prompt_events(
                &self.prefix,
                &payload.session_id,
                &payload.req_id,
            );

            self.handle_prompt(payload, events_subject).await;
        }

        info!("runner: subscription stream ended");
    }

    async fn handle_prompt(&self, payload: PromptPayload, events_subject: String) {
        // Subscribe to the cancel subject for this session so we can abort mid-run
        let cancel_subject =
            subjects::session_cancel(&self.prefix, &payload.session_id);
        let mut cancel_sub = match self.nats.subscribe(cancel_subject.clone()).await {
            Ok(s) => s,
            Err(e) => {
                warn!(subject = %cancel_subject, error = %e, "runner: could not subscribe to cancel");
                // Proceed without cancel support rather than aborting the prompt
                return self.handle_prompt_no_cancel(payload, events_subject).await;
            }
        };

        // Load session history from KV
        let mut state = match self.store.load(&payload.session_id).await {
            Ok(s) => s,
            Err(e) => {
                error!(session_id = %payload.session_id, error = %e, "runner: failed to load session");
                self.publish_error(&events_subject, format!("session load failed: {e}")).await;
                return;
            }
        };

        // Capture the first prompt as the session title (before appending the user turn)
        if state.title.is_empty() {
            let title_source = if !payload.user_message.is_empty() {
                payload.user_message.clone()
            } else {
                payload.content.iter().find_map(|b| {
                    if let acp_nats::prompt_event::UserContentBlock::Text { text } = b {
                        if !text.is_empty() { Some(text.clone()) } else { None }
                    } else {
                        None
                    }
                }).unwrap_or_default()
            };
            state.title = truncate_title(&title_source);
        }

        // Append the user turn
        state.messages.push(user_message_from_payload(&payload));

        // Channel for streaming agent events
        let (event_tx, mut event_rx) = mpsc::channel::<AgentEvent>(32);

        let tools = if state.disable_builtin_tools {
            vec![]
        } else {
            all_tool_defs().into_iter().filter(|t| t.name != "AskUserQuestion").collect()
        };
        // Build per-session agent with model + MCP overrides and permission gate
        let needs_perm = self.permission_tx.is_some() && state.mode != "bypassPermissions";
        let gateway = self.gateway_config.read().await.clone();
        let agent: Arc<AgentLoop> = {
            let needs_clone = state.model.is_some()
                || !state.mcp_servers.is_empty()
                || needs_perm
                || gateway.is_some();
            if needs_clone {
                let mut a = (*self.agent).clone();
                if let Some(ref model) = state.model {
                    a.model = model.clone();
                }
                if !state.mcp_servers.is_empty() {
                    let (mcp_defs, mcp_dispatch) =
                        build_session_mcp(&self.nats, &state.mcp_servers).await;
                    a.mcp_tool_defs.extend(mcp_defs);
                    a.mcp_dispatch.extend(mcp_dispatch);
                }
                if needs_perm {
                    if let Some(ref perm_tx) = self.permission_tx {
                        a.permission_checker = Some(Arc::new(ChannelPermissionChecker {
                            session_id: payload.session_id.clone(),
                            tx: perm_tx.clone(),
                            allowed_tools: state.allowed_tools.clone(),
                        }));
                    }
                }
                if let Some(ref gw) = gateway {
                    a.anthropic_base_url = Some(gw.base_url.clone());
                    a.anthropic_token = gw.token.clone();
                    a.anthropic_extra_headers = gw.extra_headers.clone();
                }
                Arc::new(a)
            } else {
                self.agent.clone()
            }
        };
        let messages = state.messages.clone();
        let system_prompt = state.system_prompt.clone();
        let system_prompt = if !state.additional_roots.is_empty() {
            let roots_info = state.additional_roots.iter()
                .map(|r| format!("- {r}"))
                .collect::<Vec<_>>()
                .join("\n");
            let roots_section = format!("Additional working directories:\n{roots_info}");
            match system_prompt {
                Some(s) => Some(format!("{s}\n\n{roots_section}")),
                None => Some(roots_section),
            }
        } else {
            system_prompt
        };

        // Capture context window size before agent is moved into spawn_local
        let context_window = Some(context_window_tokens(&agent.model));

        // Spawn the agent loop so we can select! against cancel
        let agent_fut = tokio::task::spawn_local(async move {
            agent.run_chat_streaming(messages, &tools, system_prompt.as_deref(), event_tx).await
        });

        // Forward streaming events to NATS while watching for cancel
        let mut final_messages: Option<Vec<trogon_agent::agent_loop::Message>> = None;
        let mut cancelled = false;
        let mut last_input_tokens: u32 = 0;
        let mut last_output_tokens: u32 = 0;
        let mut last_cache_creation_tokens: u32 = 0;
        let mut last_cache_read_tokens: u32 = 0;

        loop {
            tokio::select! {
                // Agent event available
                maybe_event = event_rx.recv() => {
                    match maybe_event {
                        Some(event) => {
                            let prompt_event = match event {
                                AgentEvent::TextDelta { text } => {
                                    PromptEvent::TextDelta { text }
                                }
                                AgentEvent::ThinkingDelta { text } => {
                                    PromptEvent::ThinkingDelta { text }
                                }
                                AgentEvent::ToolCallStarted { id, name, input } => {
                                    PromptEvent::ToolCallStarted { id, name, input }
                                }
                                AgentEvent::ToolCallFinished { id, output, exit_code, signal } => {
                                    PromptEvent::ToolCallFinished { id, output, exit_code, signal }
                                }
                                AgentEvent::SystemStatus { message } => {
                                    PromptEvent::SystemStatus { message }
                                }
                                AgentEvent::UsageSummary { input_tokens, output_tokens, cache_creation_tokens, cache_read_tokens } => {
                                    last_input_tokens = input_tokens;
                                    last_output_tokens = output_tokens;
                                    last_cache_creation_tokens = cache_creation_tokens;
                                    last_cache_read_tokens = cache_read_tokens;
                                    PromptEvent::UsageUpdate { input_tokens, output_tokens, cache_creation_tokens, cache_read_tokens, context_window }
                                }
                            };
                            self.publish_event(&events_subject, &prompt_event).await;
                        }
                        None => {
                            // Channel closed — agent loop is done; join the task
                            match agent_fut.await {
                                Ok(Ok(updated_messages)) => {
                                    final_messages = Some(updated_messages);
                                }
                                Ok(Err(trogon_agent::agent_loop::AgentError::MaxIterationsReached)) => {
                                    if last_input_tokens > 0 || last_output_tokens > 0 {
                                        self.publish_event(
                                            &events_subject,
                                            &PromptEvent::UsageUpdate {
                                                input_tokens: last_input_tokens,
                                                output_tokens: last_output_tokens,
                                                cache_creation_tokens: last_cache_creation_tokens,
                                                cache_read_tokens: last_cache_read_tokens,
                                                context_window,
                                            },
                                        ).await;
                                    }
                                    self.publish_event(
                                        &events_subject,
                                        &PromptEvent::Done { stop_reason: "max_turn_requests".to_string() },
                                    ).await;
                                }
                                Ok(Err(trogon_agent::agent_loop::AgentError::MaxTokens)) => {
                                    if last_input_tokens > 0 || last_output_tokens > 0 {
                                        self.publish_event(
                                            &events_subject,
                                            &PromptEvent::UsageUpdate {
                                                input_tokens: last_input_tokens,
                                                output_tokens: last_output_tokens,
                                                cache_creation_tokens: last_cache_creation_tokens,
                                                cache_read_tokens: last_cache_read_tokens,
                                                context_window,
                                            },
                                        ).await;
                                    }
                                    self.publish_event(
                                        &events_subject,
                                        &PromptEvent::Done { stop_reason: "max_tokens".to_string() },
                                    ).await;
                                }
                                Ok(Err(e)) => {
                                    self.publish_error(&events_subject, e.to_string()).await;
                                }
                                Err(e) => {
                                    self.publish_error(&events_subject, format!("agent task panicked: {e}")).await;
                                }
                            }
                            break;
                        }
                    }
                }

                // Cancel arrived
                _ = cancel_sub.next() => {
                    info!(session_id = %payload.session_id, "runner: cancel received");
                    cancelled = true;
                    agent_fut.abort();
                    break;
                }
            }
        }

        if cancelled {
            self.publish_event(
                &events_subject,
                &PromptEvent::Done { stop_reason: "cancelled".to_string() },
            )
            .await;
            return;
        }

        if let Some(updated_messages) = final_messages {
            state.messages = updated_messages;
            state.updated_at = crate::session_store::now_iso8601();
            if let Err(e) = self.store.save(&payload.session_id, &state).await {
                warn!(session_id = %payload.session_id, error = %e, "runner: failed to save session");
            }
            self.publish_event(
                &events_subject,
                &PromptEvent::Done { stop_reason: "end_turn".to_string() },
            )
            .await;
        }
    }

    /// Fallback path when we cannot subscribe to the cancel subject.
    async fn handle_prompt_no_cancel(&self, payload: PromptPayload, events_subject: String) {
        let mut state = match self.store.load(&payload.session_id).await {
            Ok(s) => s,
            Err(e) => {
                error!(session_id = %payload.session_id, error = %e, "runner: failed to load session");
                self.publish_error(&events_subject, format!("session load failed: {e}")).await;
                return;
            }
        };

        if state.title.is_empty() {
            let title_source = if !payload.user_message.is_empty() {
                payload.user_message.clone()
            } else {
                payload.content.iter().find_map(|b| {
                    if let acp_nats::prompt_event::UserContentBlock::Text { text } = b {
                        if !text.is_empty() { Some(text.clone()) } else { None }
                    } else {
                        None
                    }
                }).unwrap_or_default()
            };
            state.title = truncate_title(&title_source);
        }

        state.messages.push(user_message_from_payload(&payload));

        let tools = if state.disable_builtin_tools {
            vec![]
        } else {
            all_tool_defs().into_iter().filter(|t| t.name != "AskUserQuestion").collect()
        };
        let (event_tx, mut event_rx) = mpsc::channel::<AgentEvent>(32);
        let needs_perm = self.permission_tx.is_some() && state.mode != "bypassPermissions";
        let gateway = self.gateway_config.read().await.clone();
        let agent: Arc<AgentLoop> = {
            let needs_clone = state.model.is_some()
                || !state.mcp_servers.is_empty()
                || needs_perm
                || gateway.is_some();
            if needs_clone {
                let mut a = (*self.agent).clone();
                if let Some(ref model) = state.model {
                    a.model = model.clone();
                }
                if !state.mcp_servers.is_empty() {
                    let (mcp_defs, mcp_dispatch) =
                        build_session_mcp(&self.nats, &state.mcp_servers).await;
                    a.mcp_tool_defs.extend(mcp_defs);
                    a.mcp_dispatch.extend(mcp_dispatch);
                }
                if needs_perm {
                    if let Some(ref perm_tx) = self.permission_tx {
                        a.permission_checker = Some(Arc::new(ChannelPermissionChecker {
                            session_id: payload.session_id.clone(),
                            tx: perm_tx.clone(),
                            allowed_tools: state.allowed_tools.clone(),
                        }));
                    }
                }
                if let Some(ref gw) = gateway {
                    a.anthropic_base_url = Some(gw.base_url.clone());
                    a.anthropic_token = gw.token.clone();
                    a.anthropic_extra_headers = gw.extra_headers.clone();
                }
                Arc::new(a)
            } else {
                self.agent.clone()
            }
        };
        let messages = state.messages.clone();
        let system_prompt = state.system_prompt.clone();
        let system_prompt = if !state.additional_roots.is_empty() {
            let roots_info = state.additional_roots.iter()
                .map(|r| format!("- {r}"))
                .collect::<Vec<_>>()
                .join("\n");
            let roots_section = format!("Additional working directories:\n{roots_info}");
            match system_prompt {
                Some(s) => Some(format!("{s}\n\n{roots_section}")),
                None => Some(roots_section),
            }
        } else {
            system_prompt
        };

        // Capture context window size before agent is moved into spawn_local
        let context_window = Some(context_window_tokens(&agent.model));

        let agent_handle = tokio::task::spawn_local(async move {
            agent.run_chat_streaming(messages, &tools, system_prompt.as_deref(), event_tx).await
        });

        let mut last_input_tokens: u32 = 0;
        let mut last_output_tokens: u32 = 0;
        let mut last_cache_creation_tokens: u32 = 0;
        let mut last_cache_read_tokens: u32 = 0;

        while let Some(event) = event_rx.recv().await {
            let prompt_event = match event {
                AgentEvent::TextDelta { text } => PromptEvent::TextDelta { text },
                AgentEvent::ThinkingDelta { text } => PromptEvent::ThinkingDelta { text },
                AgentEvent::ToolCallStarted { id, name, input } => {
                    PromptEvent::ToolCallStarted { id, name, input }
                }
                AgentEvent::ToolCallFinished { id, output, exit_code, signal } => {
                    PromptEvent::ToolCallFinished { id, output, exit_code, signal }
                }
                AgentEvent::SystemStatus { message } => {
                    PromptEvent::SystemStatus { message }
                }
                AgentEvent::UsageSummary { input_tokens, output_tokens, cache_creation_tokens, cache_read_tokens } => {
                    last_input_tokens = input_tokens;
                    last_output_tokens = output_tokens;
                    last_cache_creation_tokens = cache_creation_tokens;
                    last_cache_read_tokens = cache_read_tokens;
                    PromptEvent::UsageUpdate { input_tokens, output_tokens, cache_creation_tokens, cache_read_tokens, context_window }
                }
            };
            self.publish_event(&events_subject, &prompt_event).await;
        }

        match agent_handle.await {
            Ok(Ok(updated_messages)) => {
                state.messages = updated_messages;
                state.updated_at = crate::session_store::now_iso8601();
                if let Err(e) = self.store.save(&payload.session_id, &state).await {
                    warn!(session_id = %payload.session_id, error = %e, "runner: failed to save session");
                }
                self.publish_event(
                    &events_subject,
                    &PromptEvent::Done { stop_reason: "end_turn".to_string() },
                ).await;
            }
            Ok(Err(trogon_agent::agent_loop::AgentError::MaxIterationsReached)) => {
                if last_input_tokens > 0 || last_output_tokens > 0 {
                    self.publish_event(
                        &events_subject,
                        &PromptEvent::UsageUpdate {
                            input_tokens: last_input_tokens,
                            output_tokens: last_output_tokens,
                            cache_creation_tokens: last_cache_creation_tokens,
                            cache_read_tokens: last_cache_read_tokens,
                            context_window,
                        },
                    ).await;
                }
                self.publish_event(
                    &events_subject,
                    &PromptEvent::Done { stop_reason: "max_turn_requests".to_string() },
                ).await;
            }
            Ok(Err(trogon_agent::agent_loop::AgentError::MaxTokens)) => {
                if last_input_tokens > 0 || last_output_tokens > 0 {
                    self.publish_event(
                        &events_subject,
                        &PromptEvent::UsageUpdate {
                            input_tokens: last_input_tokens,
                            output_tokens: last_output_tokens,
                            cache_creation_tokens: last_cache_creation_tokens,
                            cache_read_tokens: last_cache_read_tokens,
                            context_window,
                        },
                    ).await;
                }
                self.publish_event(
                    &events_subject,
                    &PromptEvent::Done { stop_reason: "max_tokens".to_string() },
                ).await;
            }
            _ => {
                self.publish_event(
                    &events_subject,
                    &PromptEvent::Done { stop_reason: "end_turn".to_string() },
                ).await;
            }
        }
    }

    async fn publish_event(&self, subject: &str, event: &PromptEvent) {
        match serde_json::to_vec(event) {
            Ok(bytes) => {
                if let Err(e) = self.nats.publish(subject.to_string(), Bytes::from(bytes)).await {
                    warn!(subject, error = %e, "runner: failed to publish event");
                }
            }
            Err(e) => {
                warn!(error = %e, "runner: failed to serialize event");
            }
        }
    }

    async fn publish_error(&self, subject: &str, message: String) {
        self.publish_event(subject, &PromptEvent::Error { message }).await;
    }
}

/// Connect to per-session MCP servers, initialize them, and return tool defs + dispatch table.
async fn build_session_mcp(
    _nats: &async_nats::Client,
    servers: &[StoredMcpServer],
) -> (Vec<ToolDef>, Vec<(String, String, Arc<trogon_mcp::McpClient>)>) {
    let http = reqwest::Client::new();
    let mut tool_defs = Vec::new();
    let mut dispatch = Vec::new();

    for server in servers {
        let client = Arc::new(trogon_mcp::McpClient::new(http.clone(), &server.url));

        if let Err(e) = client.initialize().await {
            warn!(name = %server.name, url = %server.url, error = %e, "MCP server init failed — skipping");
            continue;
        }

        match client.list_tools().await {
            Ok(tools) => {
                for tool in tools {
                    if tool.name == "AskUserQuestion" {
                        continue;
                    }
                    // Prefix the tool name with the server name to avoid collisions
                    let prefixed = format!("{}__{}", server.name, tool.name);
                    tool_defs.push(ToolDef {
                        name: prefixed.clone(),
                        description: tool.description,
                        input_schema: tool.input_schema,
                        cache_control: None,
                    });
                    dispatch.push((prefixed, tool.name, client.clone()));
                }
                info!(name = %server.name, tools = tool_defs.len(), "MCP server connected");
            }
            Err(e) => {
                warn!(name = %server.name, error = %e, "Failed to list MCP tools — skipping");
            }
        }
    }

    (tool_defs, dispatch)
}

/// Build a rich Anthropic user `Message` from a `PromptPayload`.
///
/// Converts `UserContentBlock`s to Anthropic `ContentBlock`s:
/// - `Text`         → plain text block
/// - `Image`        → base64 image block
/// - `ImageUrl`     → native URL image block
/// - `ResourceLink` → `[@name](uri)` text block
/// - `Context`      → `<context ref="uri">\n{text}\n</context>` text block
fn user_message_from_payload(payload: &PromptPayload) -> Message {
    // If the new rich content field is populated, use it; otherwise fall back to
    // the plain-text user_message (backward compatibility with older Bridge versions).
    if payload.content.is_empty() {
        return Message::user_text(&payload.user_message);
    }

    let blocks: Vec<ContentBlock> = payload
        .content
        .iter()
        .map(|block| match block {
            UserContentBlock::Text { text } => ContentBlock::Text { text: text.clone() },
            UserContentBlock::Image { data, mime_type } => ContentBlock::Image {
                source: ImageSource::Base64 {
                    media_type: mime_type.clone(),
                    data: data.clone(),
                },
            },
            UserContentBlock::ImageUrl { url } => ContentBlock::Image {
                source: ImageSource::Url { url: url.clone() },
            },
            UserContentBlock::ResourceLink { uri, name } => ContentBlock::Text {
                text: format!("[@{name}]({uri})"),
            },
            UserContentBlock::Context { uri, text } => ContentBlock::Text {
                text: format!("\n<context ref=\"{uri}\">\n{text}\n</context>"),
            },
        })
        .collect();

    Message { role: "user".to_string(), content: blocks }
}

/// Truncate a prompt to at most 256 characters for use as a session title.
fn truncate_title(text: &str) -> String {
    let no_newlines = text.replace(['\r', '\n'], " ");
    let collapsed: String = no_newlines.split_whitespace().collect::<Vec<_>>().join(" ");
    let trimmed = collapsed.trim().to_string();
    if trimmed.len() <= 256 {
        trimmed
    } else {
        format!("{}…", &trimmed[..255])
    }
}

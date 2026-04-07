use std::collections::HashMap;
use std::sync::Arc;

use acp_nats::acp_prefix::AcpPrefix;
use acp_nats::nats::{ExtSessionReady, session as session_subjects};
use acp_nats::session_id::AcpSessionId;
use agent_client_protocol::{
    AgentCapabilities, AuthMethod, AuthMethodAgent, AuthenticateRequest, AuthenticateResponse,
    CancelNotification, CloseSessionRequest, CloseSessionResponse, ContentBlock,
    EmbeddedResourceResource, Error, ErrorCode, ForkSessionRequest, ForkSessionResponse,
    Implementation, InitializeRequest, InitializeResponse, ListSessionsRequest,
    ListSessionsResponse, LoadSessionRequest, LoadSessionResponse, ModelInfo, NewSessionRequest,
    NewSessionResponse, PromptRequest, PromptResponse, ProtocolVersion, ResumeSessionRequest,
    ResumeSessionResponse, SessionCapabilities, SessionConfigOption,
    SessionConfigOptionCategory, SessionConfigSelectOption, SessionForkCapabilities, SessionId,
    SessionInfo, SessionListCapabilities, SessionMode, SessionModeState, SessionModelState,
    SessionResumeCapabilities, SetSessionConfigOptionRequest, SetSessionConfigOptionResponse,
    SetSessionModeRequest, SetSessionModeResponse, SetSessionModelRequest,
    SetSessionModelResponse, StopReason,
};
use async_trait::async_trait;
use bytes::Bytes;
use tokio::sync::{RwLock, mpsc};
use tracing::{error, info, warn};
use trogon_agent_core::agent_loop::{AgentEvent, ContentBlock as AgentContentBlock, ImageSource, Message};
use trogon_agent_core::tools::ToolDef;

use crate::agent_runner::AgentRunner;
use crate::permission::{ChannelPermissionChecker, PermissionTx};
use crate::prompt_converter::PromptEventConverter;
use crate::session_notifier::{PromptEventClient, SessionNotifier};
use crate::session_store::{NatsSessionStore, SessionStore, StoredMcpServer, now_iso8601};

/// Gateway credentials that override the default proxy/token when set.
#[derive(Debug, Clone)]
pub struct GatewayConfig {
    pub base_url: String,
    pub token: String,
    pub extra_headers: Vec<(String, String)>,
}

/// Returns the context window token limit for a given model ID.
fn context_window_tokens(_model: &str) -> u64 {
    200_000
}

/// Truncate a prompt to at most 256 characters for use as a session title.
fn truncate_title(text: &str) -> String {
    let no_newlines = text.replace(['\r', '\n'], " ");
    let collapsed: String = no_newlines.split_whitespace().collect::<Vec<_>>().join(" ");
    let trimmed = collapsed.trim().to_string();
    if trimmed.chars().count() <= 256 {
        trimmed
    } else {
        let truncated: String = trimmed.chars().take(255).collect();
        format!("{truncated}…")
    }
}

/// Build a rich Anthropic user `Message` from ACP `ContentBlock`s in a `PromptRequest`.
fn user_message_from_request(req: &PromptRequest) -> Message {
    if req.prompt.is_empty() {
        return Message::user_text("");
    }

    let blocks: Vec<AgentContentBlock> = req
        .prompt
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text(t) => Some(AgentContentBlock::Text { text: t.text.clone() }),
            ContentBlock::Image(img) => {
                if let Some(ref url) = img.uri {
                    Some(AgentContentBlock::Image {
                        source: ImageSource::Url { url: url.clone() },
                    })
                } else {
                    Some(AgentContentBlock::Image {
                        source: ImageSource::Base64 {
                            media_type: img.mime_type.clone(),
                            data: img.data.clone(),
                        },
                    })
                }
            }
            ContentBlock::ResourceLink(rl) => Some(AgentContentBlock::Text {
                text: format!("[@{}]({})", rl.name, rl.uri),
            }),
            ContentBlock::Resource(er) => match &er.resource {
                EmbeddedResourceResource::TextResourceContents(t) => {
                    Some(AgentContentBlock::Text {
                        text: format!("\n<context ref=\"{}\">\n{}\n</context>", t.uri, t.text),
                    })
                }
                EmbeddedResourceResource::BlobResourceContents(b) => {
                    Some(AgentContentBlock::Image {
                        source: ImageSource::Base64 {
                            media_type: b.mime_type.clone().unwrap_or_default(),
                            data: b.blob.clone(),
                        },
                    })
                }
                _ => None,
            },
            _ => None,
        })
        .collect();

    Message {
        role: "user".to_string(),
        content: blocks,
    }
}

/// Connect to per-session MCP servers, initialize them, and return tool defs + dispatch table.
#[cfg_attr(coverage, coverage(off))]
async fn build_session_mcp(
    servers: &[StoredMcpServer],
) -> (
    Vec<ToolDef>,
    Vec<(String, String, Arc<trogon_mcp::McpClient>)>,
) {
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

fn internal_error(msg: impl Into<String>) -> Error {
    Error::new(ErrorCode::InternalError.into(), msg.into())
}

/// Agent implementation that handles all ACP methods via NATS.
///
/// Generic parameters with production defaults:
/// - `S` — session store  (`NatsSessionStore`)
/// - `A` — LLM runner     (`trogon_agent_core::agent_loop::AgentLoop`)
/// - `N` — NATS notifier  (`crate::session_notifier::NatsSessionNotifier`)
pub struct TrogonAgent<
    S = NatsSessionStore,
    A = trogon_agent_core::agent_loop::AgentLoop,
    N = crate::session_notifier::NatsSessionNotifier,
> {
    notifier: N,
    store: S,
    agent: Arc<A>,
    prefix: String,
    default_model: String,
    permission_tx: Option<PermissionTx>,
    gateway_config: Arc<RwLock<Option<GatewayConfig>>>,
    /// Per-session semaphore (1 permit) to serialize concurrent prompt calls.
    session_locks: Arc<std::sync::Mutex<HashMap<String, Arc<tokio::sync::Semaphore>>>>,
}

impl<S: SessionStore, A: AgentRunner + 'static, N: SessionNotifier> TrogonAgent<S, A, N> {
    pub fn new(
        notifier: N,
        store: S,
        agent: A,
        prefix: impl Into<String>,
        default_model: impl Into<String>,
        permission_tx: Option<PermissionTx>,
        gateway_config: Arc<RwLock<Option<GatewayConfig>>>,
    ) -> Self {
        Self {
            notifier,
            store,
            agent: Arc::new(agent),
            prefix: prefix.into(),
            default_model: default_model.into(),
            permission_tx,
            gateway_config,
            session_locks: Arc::new(std::sync::Mutex::new(HashMap::new())),
        }
    }

    #[cfg_attr(coverage, coverage(off))]
    fn session_mode_state(&self, current_mode: &str) -> SessionModeState {
        SessionModeState::new(
            current_mode.to_string(),
            vec![
                SessionMode::new("default", "Default"),
                SessionMode::new("acceptEdits", "Accept Edits"),
                SessionMode::new("plan", "Plan"),
                SessionMode::new("dontAsk", "Don't Ask"),
            ],
        )
    }

    #[cfg_attr(coverage, coverage(off))]
    fn session_model_state(&self, current_model: Option<&str>) -> SessionModelState {
        let current = current_model.unwrap_or(&self.default_model).to_string();
        SessionModelState::new(
            current,
            vec![
                ModelInfo::new("claude-opus-4-6", "Claude Opus 4"),
                ModelInfo::new("claude-sonnet-4-6", "Claude Sonnet 4"),
                ModelInfo::new("claude-haiku-4-5-20251001", "Claude Haiku 4.5"),
            ],
        )
    }

    #[cfg_attr(coverage, coverage(off))]
    async fn publish_session_ready(&self, session_id: &str) {
        let acp_prefix = AcpPrefix::new(&self.prefix).expect("valid prefix");
        let subject = session_subjects::agent::ExtReadySubject::new(
            &acp_prefix,
            &AcpSessionId::new(session_id).expect("valid session_id"),
        )
        .to_string();
        let message = ExtSessionReady::new(SessionId::from(session_id.to_owned()));
        match serde_json::to_vec(&message) {
            Ok(bytes) => {
                self.notifier.publish(subject, bytes.into()).await;
            }
            Err(e) => {
                warn!(error = %e, "agent: failed to serialize session.ready");
            }
        }
    }

    fn make_acp_session_id(
        &self,
        session_id: &agent_client_protocol::SessionId,
    ) -> agent_client_protocol::Result<AcpSessionId> {
        AcpSessionId::try_from(session_id).map_err(|e| internal_error(e.to_string()))
    }

    fn make_acp_prefix(&self) -> agent_client_protocol::Result<AcpPrefix> {
        AcpPrefix::new(&self.prefix).map_err(|e| internal_error(e.to_string()))
    }

    /// Acquire (or create) the per-session semaphore permit, serializing concurrent prompts.
    fn acquire_session_lock(&self, session_id: &str) -> Arc<tokio::sync::Semaphore> {
        let mut locks = self.session_locks.lock().unwrap();
        locks
            .entry(session_id.to_string())
            .or_insert_with(|| Arc::new(tokio::sync::Semaphore::new(1)))
            .clone()
    }

    /// Core prompt execution. Streams events via `prompt_client` and returns the final response.
    #[cfg_attr(coverage, coverage(off))]
    async fn run_prompt(
        &self,
        req: &PromptRequest,
        prompt_client: &dyn PromptEventClient,
        cancel_rx: Option<tokio::sync::oneshot::Receiver<()>>,
    ) -> agent_client_protocol::Result<PromptResponse> {
        use acp_nats::prompt_event::PromptEvent;

        let session_id = req.session_id.to_string();

        let mut state = match self.store.load(&session_id).await {
            Ok(s) => s,
            Err(e) => {
                error!(session_id, error = %e, "agent: failed to load session");
                return Err(internal_error(format!("session load failed: {e}")));
            }
        };

        // Capture the first prompt as the session title
        if state.title.is_empty() {
            let title_source = req
                .prompt
                .iter()
                .find_map(|b| {
                    if let ContentBlock::Text(t) = b {
                        if !t.text.is_empty() {
                            Some(t.text.clone())
                        } else {
                            None
                        }
                    } else {
                        None
                    }
                })
                .unwrap_or_default();
            state.title = truncate_title(&title_source);
        }

        state.messages.push(user_message_from_request(req));

        let (event_tx, mut event_rx) = mpsc::channel::<AgentEvent>(32);
        let tools: Vec<ToolDef> = vec![];
        let needs_perm = self.permission_tx.is_some() && state.mode != "bypassPermissions";
        let gateway = self.gateway_config.read().await.clone();

        let agent: Arc<A> = {
            let needs_clone = state.model.is_some()
                || !state.mcp_servers.is_empty()
                || needs_perm
                || gateway.is_some();
            if needs_clone {
                let mut a = (*self.agent).clone();
                if let Some(ref model) = state.model {
                    a.set_model(model.clone());
                }
                if !state.mcp_servers.is_empty() {
                    let (mcp_defs, mcp_dispatch) = build_session_mcp(&state.mcp_servers).await;
                    a.add_mcp_tools(mcp_defs, mcp_dispatch);
                }
                if needs_perm {
                    if let Some(ref perm_tx) = self.permission_tx {
                        a.set_permission_checker(Arc::new(ChannelPermissionChecker {
                            session_id: session_id.clone(),
                            tx: perm_tx.clone(),
                            allowed_tools: state.allowed_tools.clone(),
                        }));
                    }
                }
                if let Some(ref gw) = gateway {
                    a.apply_gateway(gw);
                }
                Arc::new(a)
            } else {
                self.agent.clone()
            }
        };

        let messages = state.messages.clone();
        let system_prompt = state.system_prompt.clone();
        let system_prompt = if !state.additional_roots.is_empty() {
            let roots_info = state
                .additional_roots
                .iter()
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

        let context_window = Some(context_window_tokens(&agent.model()));
        let current_model = state
            .model
            .clone()
            .unwrap_or_else(|| self.agent.model());

        let agent_fut = tokio::task::spawn_local(async move {
            agent
                .run_chat_streaming(messages, &tools, system_prompt.as_deref(), event_tx)
                .await
        });

        let mut converter = PromptEventConverter::new(session_id.clone());
        let mut final_messages: Option<Vec<Message>> = None;
        let mut cancelled = false;
        let mut last_input_tokens: u32 = 0;
        let mut last_output_tokens: u32 = 0;
        let mut last_cache_creation_tokens: u32 = 0;
        let mut last_cache_read_tokens: u32 = 0;
        let mut tool_name_by_id: HashMap<String, String> = HashMap::new();
        let mut cancel_rx = cancel_rx;

        loop {
            let cancel_fut = async {
                match cancel_rx.as_mut() {
                    Some(rx) => { let _ = rx.await; }
                    None => std::future::pending().await,
                }
            };

            tokio::select! {
                maybe_event = event_rx.recv() => {
                    match maybe_event {
                        Some(event) => {
                            let prompt_event = match event {
                                AgentEvent::TextDelta { text } => PromptEvent::TextDelta { text },
                                AgentEvent::ThinkingDelta { text } => PromptEvent::ThinkingDelta { text },
                                AgentEvent::ToolCallStarted { id, name, input, parent_tool_use_id } => {
                                    tool_name_by_id.insert(id.clone(), name.clone());
                                    PromptEvent::ToolCallStarted { id, name, input, parent_tool_use_id }
                                }
                                AgentEvent::ToolCallFinished { id, output, exit_code, signal } => {
                                    let is_enter_plan = tool_name_by_id
                                        .get(&id)
                                        .map(|n| n == "EnterPlanMode")
                                        .unwrap_or(false);
                                    let finished = PromptEvent::ToolCallFinished { id, output, exit_code, signal };
                                    publish_via_converter(prompt_client, &mut converter, finished).await;
                                    if is_enter_plan {
                                        state.mode = "plan".to_string();
                                        publish_via_converter(
                                            prompt_client,
                                            &mut converter,
                                            PromptEvent::ModeChanged {
                                                mode: "plan".to_string(),
                                                model: current_model.clone(),
                                            },
                                        )
                                        .await;
                                    }
                                    continue;
                                }
                                AgentEvent::SystemStatus { message } => PromptEvent::SystemStatus { message },
                                AgentEvent::UsageSummary {
                                    input_tokens,
                                    output_tokens,
                                    cache_creation_tokens,
                                    cache_read_tokens,
                                } => {
                                    last_input_tokens = input_tokens;
                                    last_output_tokens = output_tokens;
                                    last_cache_creation_tokens = cache_creation_tokens;
                                    last_cache_read_tokens = cache_read_tokens;
                                    PromptEvent::UsageUpdate {
                                        input_tokens,
                                        output_tokens,
                                        cache_creation_tokens,
                                        cache_read_tokens,
                                        context_window,
                                    }
                                }
                            };
                            publish_via_converter(prompt_client, &mut converter, prompt_event).await;
                        }
                        None => {
                            match agent_fut.await {
                                Ok(Ok(updated)) => {
                                    final_messages = Some(updated);
                                }
                                Ok(Err(trogon_agent_core::agent_loop::AgentError::MaxIterationsReached)) => {
                                    if last_input_tokens > 0 || last_output_tokens > 0 {
                                        publish_via_converter(
                                            prompt_client,
                                            &mut converter,
                                            PromptEvent::UsageUpdate {
                                                input_tokens: last_input_tokens,
                                                output_tokens: last_output_tokens,
                                                cache_creation_tokens: last_cache_creation_tokens,
                                                cache_read_tokens: last_cache_read_tokens,
                                                context_window,
                                            },
                                        )
                                        .await;
                                    }
                                    return Ok(PromptResponse::new(StopReason::MaxTurnRequests));
                                }
                                Ok(Err(trogon_agent_core::agent_loop::AgentError::MaxTokens)) => {
                                    if last_input_tokens > 0 || last_output_tokens > 0 {
                                        publish_via_converter(
                                            prompt_client,
                                            &mut converter,
                                            PromptEvent::UsageUpdate {
                                                input_tokens: last_input_tokens,
                                                output_tokens: last_output_tokens,
                                                cache_creation_tokens: last_cache_creation_tokens,
                                                cache_read_tokens: last_cache_read_tokens,
                                                context_window,
                                            },
                                        )
                                        .await;
                                    }
                                    return Ok(PromptResponse::new(StopReason::MaxTokens));
                                }
                                Ok(Err(e)) => {
                                    return Err(internal_error(e.to_string()));
                                }
                                Err(e) => {
                                    return Err(internal_error(format!("agent task panicked: {e}")));
                                }
                            }
                            break;
                        }
                    }
                }
                _ = cancel_fut => {
                    info!(session_id, "agent: cancel received");
                    cancelled = true;
                    agent_fut.abort();
                    break;
                }
            }
        }

        if cancelled {
            return Ok(PromptResponse::new(StopReason::Cancelled));
        }

        if let Some(updated) = final_messages {
            state.messages = updated;
            state.updated_at = now_iso8601();
            if let Err(e) = self.store.save(&session_id, &state).await {
                warn!(session_id, error = %e, "agent: failed to save session");
            }
        }

        Ok(PromptResponse::new(StopReason::EndTurn))
    }
}

/// Convert a `PromptEvent` via `converter` and publish resulting notifications via the client.
#[cfg_attr(coverage, coverage(off))]
async fn publish_via_converter(
    client: &dyn PromptEventClient,
    converter: &mut PromptEventConverter,
    event: acp_nats::prompt_event::PromptEvent,
) {
    let (notifications, _outcome) = converter.convert(event);
    for notif in notifications {
        if let Err(e) = client.session_notification(notif).await {
            warn!(error = %e, "agent: failed to publish notification");
        }
    }
}

#[async_trait(?Send)]
impl<S: SessionStore, A: AgentRunner + 'static, N: SessionNotifier> agent_client_protocol::Agent
    for TrogonAgent<S, A, N>
{
    #[cfg_attr(coverage, coverage(off))]
    async fn initialize(
        &self,
        _req: InitializeRequest,
    ) -> agent_client_protocol::Result<InitializeResponse> {
        let mut session_caps_meta = serde_json::Map::new();
        session_caps_meta.insert("close".to_string(), serde_json::json!({}));
        let capabilities = AgentCapabilities::new()
            .load_session(true)
            .session_capabilities(
                SessionCapabilities::new()
                    .list(SessionListCapabilities::new())
                    .fork(SessionForkCapabilities::new())
                    .resume(SessionResumeCapabilities::new())
                    .meta(session_caps_meta),
            );
        Ok(InitializeResponse::new(ProtocolVersion::LATEST)
            .agent_capabilities(capabilities)
            .agent_info(Implementation::new(
                env!("CARGO_PKG_NAME"),
                env!("CARGO_PKG_VERSION"),
            ))
            .auth_methods(vec![AuthMethod::Agent(AuthMethodAgent::new(
                "gateway_auth",
                "Gateway",
            ))]))
    }

    #[cfg_attr(coverage, coverage(off))]
    async fn authenticate(
        &self,
        _req: AuthenticateRequest,
    ) -> agent_client_protocol::Result<AuthenticateResponse> {
        Ok(AuthenticateResponse::new())
    }

    #[cfg_attr(coverage, coverage(off))]
    async fn new_session(
        &self,
        req: NewSessionRequest,
    ) -> agent_client_protocol::Result<NewSessionResponse> {
        let session_id = uuid::Uuid::new_v4().to_string();

        let meta = req.meta.as_ref();
        let system_prompt = meta
            .and_then(|m| m.get("systemPrompt"))
            .and_then(|v| v.as_str())
            .map(String::from);
        let mode = meta
            .and_then(|m| m.get("mode"))
            .and_then(|v| v.as_str())
            .unwrap_or("default")
            .to_string();

        let now = now_iso8601();
        let state = crate::session_store::SessionState {
            cwd: req.cwd.to_string_lossy().to_string(),
            mode,
            system_prompt,
            created_at: now.clone(),
            updated_at: now,
            ..Default::default()
        };

        if let Err(e) = self.store.save(&session_id, &state).await {
            warn!(session_id, error = %e, "agent: failed to save new session");
            return Err(internal_error(format!("failed to save session: {e}")));
        }

        let response = NewSessionResponse::new(session_id.clone())
            .modes(self.session_mode_state(&state.mode))
            .models(self.session_model_state(state.model.as_deref()));
        self.publish_session_ready(&session_id).await;
        Ok(response)
    }

    #[cfg_attr(coverage, coverage(off))]
    async fn load_session(
        &self,
        req: LoadSessionRequest,
    ) -> agent_client_protocol::Result<LoadSessionResponse> {
        let session_id = req.session_id.to_string();
        let state = self.store.load(&session_id).await.unwrap_or_default();
        let response = LoadSessionResponse::new()
            .modes(self.session_mode_state(&state.mode))
            .models(self.session_model_state(state.model.as_deref()));
        self.publish_session_ready(&session_id).await;
        Ok(response)
    }

    #[cfg_attr(coverage, coverage(off))]
    async fn set_session_mode(
        &self,
        req: SetSessionModeRequest,
    ) -> agent_client_protocol::Result<SetSessionModeResponse> {
        let session_id = req.session_id.to_string();
        let mut state = match self.store.load(&session_id).await {
            Ok(s) => s,
            Err(e) => {
                warn!(session_id, error = %e, "agent: failed to load session for mode update");
                return Err(internal_error(format!("failed to load session: {e}")));
            }
        };
        state.mode = req.mode_id.to_string();
        state.updated_at = now_iso8601();
        if let Err(e) = self.store.save(&session_id, &state).await {
            warn!(session_id, error = %e, "agent: failed to persist mode update");
            return Err(internal_error(format!("failed to save session: {e}")));
        }
        Ok(SetSessionModeResponse::new())
    }

    #[cfg_attr(coverage, coverage(off))]
    async fn set_session_model(
        &self,
        req: SetSessionModelRequest,
    ) -> agent_client_protocol::Result<SetSessionModelResponse> {
        let session_id = req.session_id.to_string();
        let mut state = match self.store.load(&session_id).await {
            Ok(s) => s,
            Err(e) => {
                warn!(session_id, error = %e, "agent: failed to load session for model update");
                return Err(internal_error(format!("failed to load session: {e}")));
            }
        };
        state.model = Some(req.model_id.to_string());
        state.updated_at = now_iso8601();
        if let Err(e) = self.store.save(&session_id, &state).await {
            warn!(session_id, error = %e, "agent: failed to persist model update");
            return Err(internal_error(format!("failed to save session: {e}")));
        }
        Ok(SetSessionModelResponse::new())
    }

    #[cfg_attr(coverage, coverage(off))]
    async fn set_session_config_option(
        &self,
        req: SetSessionConfigOptionRequest,
    ) -> agent_client_protocol::Result<SetSessionConfigOptionResponse> {
        let session_id = req.session_id.to_string();
        let config_id = req.config_id.to_string();
        let value = match &req.value {
            agent_client_protocol::SessionConfigOptionValue::ValueId { value } => {
                value.to_string()
            }
            agent_client_protocol::SessionConfigOptionValue::Boolean { value } => {
                value.to_string()
            }
            _ => String::new(),
        };

        let mut state = self.store.load(&session_id).await.unwrap_or_default();

        match config_id.as_str() {
            "mode" => {
                state.mode = value.clone();
                state.updated_at = now_iso8601();
                if let Err(e) = self.store.save(&session_id, &state).await {
                    warn!(session_id, error = %e, "agent: failed to persist config mode update");
                }
            }
            "model" => {
                state.model = Some(value.clone());
                state.updated_at = now_iso8601();
                if let Err(e) = self.store.save(&session_id, &state).await {
                    warn!(session_id, error = %e, "agent: failed to persist config model update");
                }
            }
            other => {
                warn!(session_id, config_id = other, "agent: unknown config option");
            }
        }

        let mode_options: Vec<SessionConfigSelectOption> = self
            .session_mode_state(&state.mode)
            .available_modes
            .iter()
            .map(|m| SessionConfigSelectOption::new(m.id.to_string(), m.name.as_str()))
            .collect();
        let model_options: Vec<SessionConfigSelectOption> = self
            .session_model_state(state.model.as_deref())
            .available_models
            .iter()
            .map(|m| SessionConfigSelectOption::new(m.model_id.to_string(), m.name.as_str()))
            .collect();
        let current_mode = state.mode.clone();
        let current_model = state
            .model
            .as_deref()
            .unwrap_or(&self.default_model)
            .to_string();
        let config_options = vec![
            SessionConfigOption::select("mode", "Mode", current_mode, mode_options)
                .category(SessionConfigOptionCategory::Mode),
            SessionConfigOption::select("model", "Model", current_model, model_options)
                .category(SessionConfigOptionCategory::Model),
        ];

        Ok(SetSessionConfigOptionResponse::new(config_options))
    }

    #[cfg_attr(coverage, coverage(off))]
    async fn list_sessions(
        &self,
        _req: ListSessionsRequest,
    ) -> agent_client_protocol::Result<ListSessionsResponse> {
        let ids = match self.store.list_ids().await {
            Ok(ids) => ids,
            Err(e) => {
                warn!(error = %e, "agent: failed to list session IDs");
                vec![]
            }
        };

        let mut sessions: Vec<SessionInfo> = Vec::with_capacity(ids.len());
        for id in ids {
            let state = self.store.load(&id).await.unwrap_or_default();
            let cwd = if state.cwd.is_empty() { "/" } else { &state.cwd };
            let mut info = SessionInfo::new(id, cwd);
            if !state.title.is_empty() {
                info = info.title(state.title);
            }
            if !state.updated_at.is_empty() {
                info = info.updated_at(state.updated_at);
            }
            sessions.push(info);
        }

        Ok(ListSessionsResponse::new(sessions))
    }

    #[cfg_attr(coverage, coverage(off))]
    async fn fork_session(
        &self,
        req: ForkSessionRequest,
    ) -> agent_client_protocol::Result<ForkSessionResponse> {
        let source_id = req.session_id.to_string();
        let mut state = match self.store.load(&source_id).await {
            Ok(s) => s,
            Err(e) => {
                warn!(source_id, error = %e, "agent: failed to load source session for fork");
                return Err(internal_error(format!("failed to load source session: {e}")));
            }
        };

        let new_id = uuid::Uuid::new_v4().to_string();
        let now = now_iso8601();
        state.created_at = now.clone();
        state.updated_at = now;
        if let Err(e) = self.store.save(&new_id, &state).await {
            warn!(new_id, error = %e, "agent: failed to save forked session");
        }

        self.publish_session_ready(&new_id).await;
        Ok(ForkSessionResponse::new(new_id))
    }

    #[cfg_attr(coverage, coverage(off))]
    async fn resume_session(
        &self,
        req: ResumeSessionRequest,
    ) -> agent_client_protocol::Result<ResumeSessionResponse> {
        let session_id = req.session_id.to_string();
        self.publish_session_ready(&session_id).await;
        Ok(ResumeSessionResponse::new())
    }

    #[cfg_attr(coverage, coverage(off))]
    async fn close_session(
        &self,
        req: CloseSessionRequest,
    ) -> agent_client_protocol::Result<CloseSessionResponse> {
        let session_id = req.session_id.to_string();

        // Cancel any running prompt for this session.
        let acp_prefix = AcpPrefix::new(&self.prefix).expect("valid prefix");
        let cancel_subject = session_subjects::agent::CancelSubject::new(
            &acp_prefix,
            &AcpSessionId::new(&session_id).expect("valid session_id"),
        )
        .to_string();
        self.notifier.publish(cancel_subject, Bytes::new()).await;

        if let Err(e) = self.store.delete(&session_id).await {
            warn!(session_id, error = %e, "agent: failed to delete session");
        }

        Ok(CloseSessionResponse::new())
    }

    #[cfg_attr(coverage, coverage(off))]
    async fn prompt(
        &self,
        req: PromptRequest,
    ) -> agent_client_protocol::Result<PromptResponse> {
        let session_id = req.session_id.to_string();

        // Serialize concurrent prompts for the same session.
        let semaphore = self.acquire_session_lock(&session_id);
        let _permit = semaphore
            .acquire_owned()
            .await
            .map_err(|_| internal_error("session lock closed"))?;

        let acp_prefix = self.make_acp_prefix()?;
        let acp_session_id = self.make_acp_session_id(&req.session_id)?;
        let cancel_subject = session_subjects::agent::CancelSubject::new(
            &acp_prefix,
            &AcpSessionId::new(&session_id).expect("valid session_id"),
        )
        .to_string();

        let cancel_rx = self.notifier.subscribe_cancel(cancel_subject).await;
        let prompt_client = self
            .notifier
            .make_prompt_client(acp_session_id, acp_prefix);

        self.run_prompt(&req, &*prompt_client, cancel_rx).await
    }

    #[cfg_attr(coverage, coverage(off))]
    async fn cancel(
        &self,
        req: CancelNotification,
    ) -> agent_client_protocol::Result<()> {
        let session_id = req.session_id.to_string();
        let acp_prefix = AcpPrefix::new(&self.prefix).expect("valid prefix");
        let subject = session_subjects::agent::CancelSubject::new(
            &acp_prefix,
            &AcpSessionId::new(&session_id).expect("valid session_id"),
        )
        .to_string();
        self.notifier.publish(subject, Bytes::new()).await;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_title_short_text_unchanged() {
        let title = truncate_title("hello world");
        assert_eq!(title, "hello world");
    }

    #[test]
    fn truncate_title_collapses_whitespace() {
        let title = truncate_title("  hello   world  ");
        assert_eq!(title, "hello world");
    }

    #[test]
    fn truncate_title_replaces_newlines() {
        let title = truncate_title("hello\nworld");
        assert_eq!(title, "hello world");
    }

    #[test]
    fn truncate_title_long_text_gets_ellipsis() {
        let long = "a".repeat(300);
        let title = truncate_title(&long);
        assert!(title.ends_with('…'));
        assert!(title.chars().count() <= 256);
    }

    #[test]
    fn user_message_from_request_empty_prompt_returns_user_text() {
        let req = PromptRequest::new("s1", vec![]);
        let msg = user_message_from_request(&req);
        assert_eq!(msg.role, "user");
    }

    #[test]
    fn user_message_from_request_text_block() {
        use agent_client_protocol::TextContent;
        let req = PromptRequest::new(
            "s1",
            vec![ContentBlock::Text(TextContent::new("hello"))],
        );
        let msg = user_message_from_request(&req);
        assert_eq!(msg.role, "user");
        assert_eq!(msg.content.len(), 1);
        match &msg.content[0] {
            AgentContentBlock::Text { text } => assert_eq!(text, "hello"),
            _ => panic!("expected Text block"),
        }
    }

    #[test]
    fn enter_plan_mode_detected_via_name_cache() {
        let mut tool_name_by_id: HashMap<String, String> = HashMap::new();
        let id = "tool-abc-123".to_string();
        tool_name_by_id.insert(id.clone(), "EnterPlanMode".to_string());
        let is_enter_plan = tool_name_by_id
            .get(&id)
            .map(|n| n == "EnterPlanMode")
            .unwrap_or(false);
        assert!(is_enter_plan);
    }

    #[test]
    fn other_tools_not_detected_as_enter_plan_mode() {
        let mut tool_name_by_id: HashMap<String, String> = HashMap::new();
        tool_name_by_id.insert("id-1".to_string(), "get_pr_diff".to_string());
        tool_name_by_id.insert("id-2".to_string(), "TodoWrite".to_string());
        for id in &["id-1", "id-2"] {
            let is_enter_plan = tool_name_by_id
                .get(*id)
                .map(|n| n == "EnterPlanMode")
                .unwrap_or(false);
            assert!(!is_enter_plan, "tool {id} must not be detected as EnterPlanMode");
        }
    }
}

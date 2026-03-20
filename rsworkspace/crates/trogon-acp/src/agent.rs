//! `TrogonAcpAgent` — local implementation of the ACP [`Agent`] trait.
//!
//! Handles all lifecycle methods locally.  Delegates `prompt` and `cancel`
//! to the inner [`Bridge`], which routes them through NATS to the Runner.

use std::path::PathBuf;
use std::time::Duration;

use agent_client_protocol::{
    AgentCapabilities, AuthMethod, AuthenticateRequest, AuthenticateResponse, AvailableCommand,
    AvailableCommandsUpdate, CancelNotification, ConfigOptionUpdate, ContentBlock, ContentChunk,
    CurrentModeUpdate, Error, ErrorCode, ExtNotification, ExtRequest, ExtResponse,
    ForkSessionRequest, ForkSessionResponse, Implementation, InitializeRequest, InitializeResponse,
    ListSessionsRequest, ListSessionsResponse, LoadSessionRequest, LoadSessionResponse,
    McpCapabilities, ModelInfo, NewSessionRequest, NewSessionResponse, Plan, PlanEntry,
    PlanEntryPriority, PlanEntryStatus, PromptCapabilities, PromptRequest, PromptResponse,
    ProtocolVersion, Result, ResumeSessionRequest, ResumeSessionResponse, SessionCapabilities,
    SessionConfigOption, SessionConfigOptionCategory, SessionForkCapabilities, SessionId,
    SessionInfo, SessionListCapabilities, SessionMode, SessionModeState, SessionModelState,
    SessionNotification, SessionResumeCapabilities, SessionUpdate, SetSessionConfigOptionRequest,
    SetSessionConfigOptionResponse, SetSessionModeRequest, SetSessionModeResponse,
    SetSessionModelRequest, SetSessionModelResponse, TextContent, ToolCall, ToolCallStatus,
    ToolCallUpdate, ToolCallUpdateFields,
};
use tokio::sync::{RwLock, mpsc};
use tracing::{info, warn};

use acp_nats::Bridge;
use acp_nats::nats::{FlushClient, PublishClient, RequestClient, SubscribeClient};
use agent_client_protocol::McpServer;
use trogon_acp_runner::{GatewayConfig, SessionState, SessionStore, StoredMcpServer};
use trogon_agent_core::agent_loop::ContentBlock as AgentContentBlock;
use trogon_std::time::GetElapsed;

const SESSION_READY_DELAY: Duration = Duration::from_millis(100);

/// Hardcoded available Claude models exposed by this agent.
const AVAILABLE_MODELS: &[(&str, &str)] = &[
    ("claude-opus-4-6", "Claude Opus 4"),
    ("claude-sonnet-4-6", "Claude Sonnet 4"),
    ("claude-haiku-4-5-20251001", "Claude Haiku 4.5"),
];

/// ACP `Agent` implementation that handles lifecycle methods locally and
/// routes `prompt`/`cancel` through NATS via the inner `Bridge`.
pub struct TrogonAcpAgent<N, C>
where
    N: RequestClient + PublishClient + SubscribeClient + FlushClient,
    C: GetElapsed,
{
    pub(crate) bridge: Bridge<N, C>,
    pub(crate) store: SessionStore,
    pub(crate) nats: async_nats::Client,
    pub(crate) prefix: String,
    pub(crate) notification_sender: mpsc::Sender<SessionNotification>,
    /// Default model configured for this agent instance (from AGENT_MODEL env var).
    pub(crate) default_model: String,
    /// Shared gateway config — written by `authenticate()`, read by the Runner.
    pub(crate) gateway_config: std::sync::Arc<RwLock<Option<GatewayConfig>>>,
}

impl<N, C> TrogonAcpAgent<N, C>
where
    N: RequestClient + PublishClient + SubscribeClient + FlushClient,
    C: GetElapsed,
{
    pub fn new(
        bridge: Bridge<N, C>,
        store: SessionStore,
        nats: async_nats::Client,
        prefix: impl Into<String>,
        notification_sender: mpsc::Sender<SessionNotification>,
        default_model: impl Into<String>,
        gateway_config: std::sync::Arc<RwLock<Option<GatewayConfig>>>,
    ) -> Self {
        Self {
            bridge,
            store,
            nats,
            prefix: prefix.into(),
            notification_sender,
            default_model: default_model.into(),
            gateway_config,
        }
    }

    /// Build the `SessionModeState` for a session.
    fn build_mode_state(current_mode: &str, allow_bypass: bool) -> SessionModeState {
        let mut modes = vec![
            SessionMode::new("default", "Default").description("Standard behavior"),
            SessionMode::new("acceptEdits", "Accept Edits")
                .description("Auto-accept file edit operations"),
            SessionMode::new("plan", "Plan Mode")
                .description("Planning mode, no actual tool execution"),
            SessionMode::new("dontAsk", "Don't Ask").description("Don't prompt for permissions"),
        ];
        if allow_bypass {
            modes.push(
                SessionMode::new("bypassPermissions", "Bypass Permissions")
                    .description("Bypass all permission checks"),
            );
        }
        SessionModeState::new(current_mode.to_string(), modes)
    }

    /// Build the `SessionModelState` for a session.
    fn build_model_state(current_model: &str) -> SessionModelState {
        let available = AVAILABLE_MODELS
            .iter()
            .map(|(id, name)| ModelInfo::new(*id, *name))
            .collect();
        SessionModelState::new(current_model.to_string(), available)
    }

    /// Build the `SessionConfigOption` list for a session.
    pub(crate) fn build_config_options(
        current_mode: &str,
        current_model: &str,
        allow_bypass: bool,
    ) -> Vec<SessionConfigOption> {
        use agent_client_protocol::SessionConfigSelectOption;
        let mut mode_options: Vec<SessionConfigSelectOption> = vec![
            SessionConfigSelectOption::new("default", "Default"),
            SessionConfigSelectOption::new("acceptEdits", "Accept Edits"),
            SessionConfigSelectOption::new("plan", "Plan Mode"),
            SessionConfigSelectOption::new("dontAsk", "Don't Ask"),
        ];
        if allow_bypass {
            mode_options.push(SessionConfigSelectOption::new(
                "bypassPermissions",
                "Bypass Permissions",
            ));
        }
        let model_options: Vec<SessionConfigSelectOption> = AVAILABLE_MODELS
            .iter()
            .map(|(id, name)| SessionConfigSelectOption::new(*id, *name))
            .collect();

        vec![
            SessionConfigOption::select("mode", "Mode", current_mode.to_string(), mode_options)
                .category(SessionConfigOptionCategory::Mode),
            SessionConfigOption::select("model", "Model", current_model.to_string(), model_options)
                .category(SessionConfigOptionCategory::Model),
        ]
    }

    async fn publish_session_ready(&self, session_id: &str) {
        let nats = self.nats.clone();
        let subject = format!("{}.{}.agent.ext.session.ready", self.prefix, session_id);
        let body =
            serde_json::to_vec(&serde_json::json!({ "sessionId": session_id })).unwrap_or_default();

        tokio::spawn(async move {
            tokio::time::sleep(SESSION_READY_DELAY).await;
            if let Err(e) = nats.publish(subject.clone(), body.into()).await {
                warn!(subject = %subject, error = %e, "Failed to publish session.ready");
            }
        });
    }

    /// Send an `available_commands_update` notification asynchronously.
    /// Builds one slash command entry per MCP server (e.g. `"myserver:"`).
    async fn send_available_commands_update(
        &self,
        session_id: &SessionId,
        mcp_servers: &[trogon_acp_runner::StoredMcpServer],
    ) {
        let commands: Vec<AvailableCommand> = mcp_servers
            .iter()
            .map(|s| {
                AvailableCommand::new(
                    format!("{}:", s.name),
                    format!("Commands provided by MCP server '{}'", s.name),
                )
            })
            .collect();
        let notification = SessionNotification::new(
            session_id.clone(),
            SessionUpdate::AvailableCommandsUpdate(AvailableCommandsUpdate::new(commands)),
        );
        let sender = self.notification_sender.clone();
        let sid = session_id.clone();
        tokio::spawn(async move {
            if sender.send(notification).await.is_err() {
                warn!(session_id = %sid, "notification receiver dropped sending available_commands");
            }
        });
    }

    /// Replay session history as ACP notifications.
    ///
    /// - User messages (simple text): skipped
    /// - Assistant text: `AgentMessageChunk`
    /// - Assistant tool_use: `ToolCall` (InProgress → Completed)
    /// - User tool_result: `ToolCallUpdate` (Completed)
    async fn replay_history(&self, session_id: &SessionId, state: &SessionState) {
        // Track TodoWrite tool-use ids so we skip their tool_result replays
        let mut todo_write_ids: std::collections::HashSet<String> =
            std::collections::HashSet::new();

        for msg in &state.messages {
            match msg.role.as_str() {
                "assistant" => {
                    for block in &msg.content {
                        match block {
                            AgentContentBlock::Text { text } if !text.is_empty() => {
                                let n = SessionNotification::new(
                                    session_id.clone(),
                                    SessionUpdate::AgentMessageChunk(ContentChunk::new(
                                        ContentBlock::Text(TextContent::new(text.clone())),
                                    )),
                                );
                                if self.notification_sender.send(n).await.is_err() {
                                    return;
                                }
                            }
                            AgentContentBlock::Thinking { thinking } if !thinking.is_empty() => {
                                let n = SessionNotification::new(
                                    session_id.clone(),
                                    SessionUpdate::AgentThoughtChunk(ContentChunk::new(
                                        ContentBlock::Text(TextContent::new(thinking.clone())),
                                    )),
                                );
                                if self.notification_sender.send(n).await.is_err() {
                                    return;
                                }
                            }
                            AgentContentBlock::ToolUse { id, name, input } => {
                                // TodoWrite → replay as Plan update, not a tool_call
                                if name == "TodoWrite"
                                    && let Some(entries) = replay_todo_write_to_plan(input)
                                {
                                    todo_write_ids.insert(id.clone());
                                    let n = SessionNotification::new(
                                        session_id.clone(),
                                        SessionUpdate::Plan(Plan::new(entries)),
                                    );
                                    if self.notification_sender.send(n).await.is_err() {
                                        return;
                                    }
                                    continue;
                                }
                                // Standard tool — show as InProgress then Completed
                                let mut meta = serde_json::Map::new();
                                let mut cc = serde_json::Map::new();
                                cc.insert(
                                    "toolName".to_string(),
                                    serde_json::Value::String(name.clone()),
                                );
                                meta.insert(
                                    "claudeCode".to_string(),
                                    serde_json::Value::Object(cc),
                                );
                                let tool_call = ToolCall::new(id.clone(), name.clone())
                                    .status(ToolCallStatus::InProgress)
                                    .raw_input(input.clone())
                                    .meta(meta);
                                let n = SessionNotification::new(
                                    session_id.clone(),
                                    SessionUpdate::ToolCall(tool_call),
                                );
                                if self.notification_sender.send(n).await.is_err() {
                                    return;
                                }
                            }
                            _ => {}
                        }
                    }
                }
                "user" => {
                    for block in &msg.content {
                        if let AgentContentBlock::ToolResult {
                            tool_use_id,
                            content,
                        } = block
                        {
                            // Skip result for TodoWrite — Plan was already replayed
                            if todo_write_ids.contains(tool_use_id) {
                                continue;
                            }
                            let fields = ToolCallUpdateFields::new()
                                .status(ToolCallStatus::Completed)
                                .raw_output(serde_json::Value::String(content.clone()));
                            let update = ToolCallUpdate::new(tool_use_id.clone(), fields);
                            let n = SessionNotification::new(
                                session_id.clone(),
                                SessionUpdate::ToolCallUpdate(update),
                            );
                            if self.notification_sender.send(n).await.is_err() {
                                return;
                            }
                        }
                        // Simple user text messages are skipped (matching TS behaviour)
                    }
                }
                _ => {}
            }
        }
    }

    /// Resolve a model string to a known model ID using fuzzy matching.
    ///
    /// Algorithm (same as TypeScript `resolveModelPreference`):
    /// 1. Exact match on ID
    /// 2. Case-insensitive match on display name
    /// 3. Substring match (id/name contains query, or query contains id)
    /// 4. Tokenized match — split by non-alphanumeric, score by token overlap
    fn resolve_model(preference: &str) -> Option<&'static str> {
        let trimmed = preference.trim();
        if trimmed.is_empty() {
            return None;
        }
        let lower = trimmed.to_lowercase();

        // 1. Exact ID match
        if let Some((id, _)) = AVAILABLE_MODELS.iter().find(|(id, _)| *id == trimmed) {
            return Some(id);
        }
        // 2. Case-insensitive ID or name match
        if let Some((id, _)) = AVAILABLE_MODELS
            .iter()
            .find(|(id, name)| id.to_lowercase() == lower || name.to_lowercase() == lower)
        {
            return Some(id);
        }
        // 3. Substring match
        if let Some((id, _)) = AVAILABLE_MODELS.iter().find(|(id, name)| {
            let il = id.to_lowercase();
            let nl = name.to_lowercase();
            il.contains(&lower) || nl.contains(&lower) || lower.contains(il.as_str())
        }) {
            return Some(id);
        }
        // 4. Tokenized match — "opus" → "claude-opus-4-6"
        let tokens: Vec<&str> = lower
            .split(|c: char| !c.is_alphanumeric())
            .filter(|s| !s.is_empty() && *s != "claude")
            .collect();
        if tokens.is_empty() {
            return None;
        }
        let mut best: Option<&'static str> = None;
        let mut best_score = 0usize;
        for (id, name) in AVAILABLE_MODELS {
            let haystack = format!("{} {}", id.to_lowercase(), name.to_lowercase());
            let score = tokens.iter().filter(|&&t| haystack.contains(t)).count();
            if score > best_score {
                best_score = score;
                best = Some(id);
            }
        }
        if best_score > 0 { best } else { None }
    }

    /// Convert ACP `McpServer` list to storable configs (Http/Sse only; stdio skipped).
    fn convert_mcp_servers(servers: &[McpServer]) -> Vec<StoredMcpServer> {
        servers
            .iter()
            .filter_map(|s| match s {
                McpServer::Http(h) => Some(StoredMcpServer {
                    name: h.name.clone(),
                    url: h.url.clone(),
                    headers: h
                        .headers
                        .iter()
                        .map(|hv| (hv.name.clone(), hv.value.clone()))
                        .collect(),
                }),
                McpServer::Sse(s) => Some(StoredMcpServer {
                    name: s.name.clone(),
                    url: s.url.clone(),
                    headers: s
                        .headers
                        .iter()
                        .map(|hv| (hv.name.clone(), hv.value.clone()))
                        .collect(),
                }),
                _ => None, // Stdio not supported in NATS model
            })
            .collect()
    }

    /// Delete a session from KV and publish a cancel to abort any running prompt.
    async fn close_session_impl(&self, session_id: &str) {
        let cancel_subject = acp_nats::nats::agent::session_cancel(&self.prefix, session_id);
        let empty: Vec<u8> = vec![];
        let _ = self.nats.publish(cancel_subject, empty.into()).await;
        let cancelled_subject = acp_nats::nats::agent::session_cancelled(&self.prefix, session_id);
        let empty: Vec<u8> = vec![];
        let _ = self.nats.publish(cancelled_subject, empty.into()).await;
        if let Err(e) = self.store.delete(session_id).await {
            warn!(session_id, error = %e, "Failed to delete session on close");
        }
    }
}

#[async_trait::async_trait(?Send)]
impl<N, C> agent_client_protocol::Agent for TrogonAcpAgent<N, C>
where
    N: RequestClient
        + PublishClient
        + SubscribeClient
        + FlushClient
        + Clone
        + Send
        + Sync
        + 'static,
    C: GetElapsed + Send + Sync + 'static,
{
    async fn initialize(&self, args: InitializeRequest) -> Result<InitializeResponse> {
        let client = args
            .client_info
            .as_ref()
            .map(|c| c.name.as_str())
            .unwrap_or("unknown");
        info!(client = %client, "ACP initialize");

        let mut caps_meta = serde_json::Map::new();
        // Advertise `close` capability — not yet a first-class field in the Rust SDK
        caps_meta.insert("close".to_string(), serde_json::json!({}));

        let session_caps = SessionCapabilities::new()
            .list(SessionListCapabilities::new())
            .fork(SessionForkCapabilities::new())
            .resume(SessionResumeCapabilities::new())
            .meta(caps_meta);

        let mut meta = serde_json::Map::new();
        meta.insert(
            "claudeCode".to_string(),
            serde_json::json!({ "promptQueueing": true }),
        );

        Ok(InitializeResponse::new(ProtocolVersion::LATEST)
            .agent_capabilities(
                AgentCapabilities::new()
                    .load_session(true)
                    .session_capabilities(session_caps)
                    .prompt_capabilities(
                        PromptCapabilities::new().image(true).embedded_context(true),
                    )
                    .mcp_capabilities(McpCapabilities::new().http(true).sse(true))
                    .meta(meta),
            )
            .auth_methods(vec![
                AuthMethod::new("gateway", "Model Gateway")
                    .description("Connect via a custom Anthropic-compatible gateway"),
            ])
            .agent_info(Implementation::new("trogon-acp", "0.1.0").title("Claude Agent")))
    }

    async fn authenticate(&self, args: AuthenticateRequest) -> Result<AuthenticateResponse> {
        // Only the "gateway" auth method is supported.
        if args.method_id.0.as_ref() != "gateway" {
            return Err(Error::new(
                ErrorCode::InvalidParams.into(),
                format!("unsupported auth method: {}", args.method_id.0),
            ));
        }

        // _meta shape: { "gateway": { "baseUrl": "...", "headers": { "Authorization": "Bearer ..." } } }
        let gateway = args
            .meta
            .as_ref()
            .and_then(|m| m.get("gateway"))
            .and_then(|v| v.as_object());

        if let Some(gw) = gateway {
            let url = gw.get("baseUrl").and_then(|v| v.as_str());
            if let Some(url) = url {
                // headers is a flat Record<string, string>
                let extra_headers: Vec<(String, String)> = gw
                    .get("headers")
                    .and_then(|v| v.as_object())
                    .map(|map| {
                        map.iter()
                            .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                            .collect()
                    })
                    .unwrap_or_default();

                // Derive the auth token from the Authorization header if present,
                // falling back to an empty string (gateway may use header-based auth).
                let token = extra_headers
                    .iter()
                    .find(|(k, _)| k.eq_ignore_ascii_case("authorization"))
                    .map(|(_, v)| v.strip_prefix("Bearer ").unwrap_or(v).to_string())
                    .unwrap_or_default();

                info!(gateway_url = %url, "authenticate: gateway config set");
                *self.gateway_config.write().await = Some(GatewayConfig {
                    base_url: url.to_string(),
                    token,
                    extra_headers,
                });
            }
        }

        Ok(AuthenticateResponse::new())
    }

    async fn new_session(&self, args: NewSessionRequest) -> Result<NewSessionResponse> {
        let session_id = uuid::Uuid::new_v4().to_string();
        info!(session_id = %session_id, cwd = ?args.cwd, "New ACP session");

        let cwd = args.cwd.to_string_lossy().to_string();
        let system_prompt = args
            .meta
            .as_ref()
            .and_then(|m| m.get("systemPrompt"))
            .and_then(|v| {
                v.as_str()
                    .or_else(|| v.get("append").and_then(|a| a.as_str()))
                    .map(|s| s.to_string())
            });
        let additional_roots: Vec<String> = args
            .meta
            .as_ref()
            .and_then(|m| m.get("additionalRoots"))
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|e| e.as_str().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default();
        let disable_builtin_tools = args
            .meta
            .as_ref()
            .and_then(|m| m.get("disableBuiltInTools"))
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let state = SessionState {
            cwd,
            created_at: now_iso8601(),
            mode: "default".to_string(),
            mcp_servers: Self::convert_mcp_servers(&args.mcp_servers),
            system_prompt,
            additional_roots,
            disable_builtin_tools,
            ..Default::default()
        };
        if let Err(e) = self.store.save(&session_id, &state).await {
            warn!(session_id = %session_id, error = %e, "Failed to initialise session KV");
        }

        let sid = SessionId::from(session_id.clone());
        self.publish_session_ready(&session_id).await;
        self.send_available_commands_update(&sid, &state.mcp_servers)
            .await;

        let allow_bypass = !is_running_as_root();
        let modes = Self::build_mode_state(&state.mode, allow_bypass);
        let models = Self::build_model_state(&self.default_model);
        let config_options =
            Self::build_config_options(&state.mode, &self.default_model, allow_bypass);

        Ok(NewSessionResponse::new(sid)
            .modes(modes)
            .models(models)
            .config_options(config_options))
    }

    async fn load_session(&self, args: LoadSessionRequest) -> Result<LoadSessionResponse> {
        let session_id = args.session_id.to_string();
        info!(session_id = %session_id, "Load ACP session");

        let state = self.store.load(&session_id).await.map_err(|e| {
            Error::new(
                ErrorCode::InternalError.into(),
                format!("Failed to load session: {e}"),
            )
        })?;

        self.replay_history(&args.session_id, &state).await;
        self.publish_session_ready(&session_id).await;
        self.send_available_commands_update(&args.session_id, &state.mcp_servers)
            .await;

        let current_mode = if state.mode.is_empty() {
            "default"
        } else {
            &state.mode
        };
        let current_model = state.model.as_deref().unwrap_or(&self.default_model);

        let allow_bypass = !is_running_as_root();
        let modes = Self::build_mode_state(current_mode, allow_bypass);
        let models = Self::build_model_state(current_model);
        let config_options = Self::build_config_options(current_mode, current_model, allow_bypass);

        Ok(LoadSessionResponse::new()
            .modes(modes)
            .models(models)
            .config_options(config_options))
    }

    async fn set_session_mode(
        &self,
        args: SetSessionModeRequest,
    ) -> Result<SetSessionModeResponse> {
        let session_id = args.session_id.to_string();
        let mode_id = args.mode_id.to_string();
        info!(session_id = %session_id, mode = %mode_id, "Set session mode");

        const VALID_MODES: &[&str] = &[
            "default",
            "acceptEdits",
            "plan",
            "dontAsk",
            "bypassPermissions",
        ];
        if !VALID_MODES.contains(&mode_id.as_str()) {
            return Err(Error::new(
                ErrorCode::InvalidParams.into(),
                format!("Invalid mode: {mode_id}"),
            ));
        }
        if mode_id == "bypassPermissions" && is_running_as_root() {
            return Err(Error::new(
                ErrorCode::InvalidParams.into(),
                "bypassPermissions cannot be used when running as root or with sudo",
            ));
        }

        let mut state = self.store.load(&session_id).await.map_err(|e| {
            Error::new(
                ErrorCode::InternalError.into(),
                format!("Failed to load session: {e}"),
            )
        })?;
        state.mode = mode_id.clone();
        if let Err(e) = self.store.save(&session_id, &state).await {
            warn!(session_id, error = %e, "Failed to save session mode");
        }

        let current_model = state.model.as_deref().unwrap_or(&self.default_model);

        // Notify client of mode change
        let mode_notification = SessionNotification::new(
            args.session_id.clone(),
            SessionUpdate::CurrentModeUpdate(CurrentModeUpdate::new(mode_id.clone())),
        );
        let _ = self.notification_sender.send(mode_notification).await;

        // Send updated config options
        let config_options =
            Self::build_config_options(&mode_id, current_model, !is_running_as_root());
        let config_notification = SessionNotification::new(
            args.session_id.clone(),
            SessionUpdate::ConfigOptionUpdate(ConfigOptionUpdate::new(config_options)),
        );
        let _ = self.notification_sender.send(config_notification).await;

        Ok(SetSessionModeResponse::new())
    }

    async fn set_session_config_option(
        &self,
        args: SetSessionConfigOptionRequest,
    ) -> Result<SetSessionConfigOptionResponse> {
        let session_id = args.session_id.to_string();
        let config_id = args.config_id.0.as_ref();
        let value = args.value.0.to_string();

        let mut state = self.store.load(&session_id).await.map_err(|e| {
            Error::new(
                ErrorCode::InternalError.into(),
                format!("Failed to load session: {e}"),
            )
        })?;

        if config_id == "mode" {
            const VALID_MODES: &[&str] = &[
                "default",
                "acceptEdits",
                "plan",
                "dontAsk",
                "bypassPermissions",
            ];
            if !VALID_MODES.contains(&value.as_str()) {
                return Err(Error::new(
                    ErrorCode::InvalidParams.into(),
                    format!("Invalid mode: {value}"),
                ));
            }
            if value == "bypassPermissions" && is_running_as_root() {
                return Err(Error::new(
                    ErrorCode::InvalidParams.into(),
                    "bypassPermissions cannot be used when running as root or with sudo",
                ));
            }
            state.mode = value.clone();
            if let Err(e) = self.store.save(&session_id, &state).await {
                warn!(session_id, error = %e, "Failed to save session mode");
            }
            let notification = SessionNotification::new(
                args.session_id.clone(),
                SessionUpdate::CurrentModeUpdate(CurrentModeUpdate::new(value.clone())),
            );
            let _ = self.notification_sender.send(notification).await;
        } else if config_id == "model" {
            let resolved = Self::resolve_model(&value).ok_or_else(|| {
                Error::new(
                    ErrorCode::InvalidParams.into(),
                    format!("Unknown model: {value}"),
                )
            })?;
            state.model = Some(resolved.to_string());
            if let Err(e) = self.store.save(&session_id, &state).await {
                warn!(session_id, error = %e, "Failed to save session model");
            }
        }

        let current_mode = if state.mode.is_empty() {
            "default"
        } else {
            &state.mode
        };
        let current_model = state.model.as_deref().unwrap_or(&self.default_model);
        let config_options =
            Self::build_config_options(current_mode, current_model, !is_running_as_root());

        let config_notification = SessionNotification::new(
            args.session_id.clone(),
            SessionUpdate::ConfigOptionUpdate(ConfigOptionUpdate::new(config_options.clone())),
        );
        let _ = self.notification_sender.send(config_notification).await;

        Ok(SetSessionConfigOptionResponse::new(config_options))
    }

    async fn set_session_model(
        &self,
        args: SetSessionModelRequest,
    ) -> Result<SetSessionModelResponse> {
        let session_id = args.session_id.to_string();
        let raw_model = args.model_id.0.to_string();
        let model = Self::resolve_model(&raw_model)
            .map(|s| s.to_string())
            .unwrap_or(raw_model);
        info!(session_id = %session_id, model = %model, "Set session model");

        let mut state = self.store.load(&session_id).await.map_err(|e| {
            Error::new(
                ErrorCode::InternalError.into(),
                format!("Failed to load session: {e}"),
            )
        })?;
        state.model = Some(model.clone());
        if let Err(e) = self.store.save(&session_id, &state).await {
            warn!(session_id, error = %e, "Failed to save session model");
        }

        let current_mode = if state.mode.is_empty() {
            "default"
        } else {
            &state.mode
        };
        let config_options =
            Self::build_config_options(current_mode, &model, !is_running_as_root());
        let config_notification = SessionNotification::new(
            args.session_id.clone(),
            SessionUpdate::ConfigOptionUpdate(ConfigOptionUpdate::new(config_options)),
        );
        let _ = self.notification_sender.send(config_notification).await;

        Ok(SetSessionModelResponse::new())
    }

    async fn list_sessions(&self, args: ListSessionsRequest) -> Result<ListSessionsResponse> {
        let ids = self.store.list_ids().await.map_err(|e| {
            Error::new(
                ErrorCode::InternalError.into(),
                format!("Failed to list sessions: {e}"),
            )
        })?;

        let mut sessions = Vec::with_capacity(ids.len());
        for id in &ids {
            let state = self.store.load(id).await.unwrap_or_default();
            // cwd filter: if the caller supplied a directory, only return sessions under it
            let requested_cwd_buf;
            let requested_cwd = match &args.cwd {
                Some(p) => {
                    requested_cwd_buf = p.to_string_lossy();
                    requested_cwd_buf.as_ref()
                }
                None => "",
            };
            if !requested_cwd.is_empty()
                && requested_cwd != "/"
                && !state.cwd.starts_with(requested_cwd)
            {
                continue;
            }
            if state.cwd.is_empty() {
                continue;
            }
            let cwd = PathBuf::from(&state.cwd);
            let mut info = SessionInfo::new(id.clone(), cwd);
            let ts = if !state.updated_at.is_empty() {
                &state.updated_at
            } else {
                &state.created_at
            };
            if !ts.is_empty() {
                info = info.updated_at(ts.clone());
            }
            if !state.title.is_empty() {
                let sanitized = sanitize_title(&state.title);
                if !sanitized.is_empty() {
                    info = info.title(sanitized);
                }
            }
            sessions.push(info);
        }
        Ok(ListSessionsResponse::new(sessions))
    }

    async fn fork_session(&self, args: ForkSessionRequest) -> Result<ForkSessionResponse> {
        let src_id = args.session_id.to_string();
        info!(src_session_id = %src_id, "Fork ACP session");

        let src_state = self.store.load(&src_id).await.map_err(|e| {
            Error::new(
                ErrorCode::InternalError.into(),
                format!("Failed to load source session: {e}"),
            )
        })?;

        let new_id = uuid::Uuid::new_v4().to_string();
        let cwd = args.cwd.to_string_lossy().to_string();
        let system_prompt = args
            .meta
            .as_ref()
            .and_then(|m| m.get("systemPrompt"))
            .and_then(|v| {
                v.as_str()
                    .or_else(|| v.get("append").and_then(|a| a.as_str()))
                    .map(|s| s.to_string())
            })
            .or_else(|| src_state.system_prompt.clone());
        let additional_roots: Vec<String> = args
            .meta
            .as_ref()
            .and_then(|m| m.get("additionalRoots"))
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|e| e.as_str().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_else(|| src_state.additional_roots.clone());
        let disable_builtin_tools = args
            .meta
            .as_ref()
            .and_then(|m| m.get("disableBuiltInTools"))
            .and_then(|v| v.as_bool())
            .unwrap_or(src_state.disable_builtin_tools);
        let new_state = SessionState {
            messages: src_state.messages.clone(),
            model: src_state.model.clone(),
            mode: src_state.mode.clone(),
            cwd,
            created_at: now_iso8601(),
            updated_at: now_iso8601(),
            title: src_state.title.clone(),
            mcp_servers: Self::convert_mcp_servers(&args.mcp_servers),
            system_prompt,
            additional_roots,
            disable_builtin_tools,
            allowed_tools: src_state.allowed_tools.clone(),
        };
        if let Err(e) = self.store.save(&new_id, &new_state).await {
            warn!(session_id = %new_id, error = %e, "Failed to save forked session");
        }

        let sid = SessionId::from(new_id.clone());
        self.publish_session_ready(&new_id).await;
        self.send_available_commands_update(&sid, &new_state.mcp_servers)
            .await;

        let current_mode = if new_state.mode.is_empty() {
            "default"
        } else {
            &new_state.mode
        };
        let current_model = new_state.model.as_deref().unwrap_or(&self.default_model);

        let allow_bypass = !is_running_as_root();
        Ok(ForkSessionResponse::new(sid)
            .modes(Self::build_mode_state(current_mode, allow_bypass))
            .models(Self::build_model_state(current_model))
            .config_options(Self::build_config_options(
                current_mode,
                current_model,
                allow_bypass,
            )))
    }

    async fn resume_session(&self, args: ResumeSessionRequest) -> Result<ResumeSessionResponse> {
        let session_id = args.session_id.to_string();
        info!(session_id = %session_id, "Resume ACP session");

        let state = self.store.load(&session_id).await.map_err(|e| {
            Error::new(
                ErrorCode::InternalError.into(),
                format!("Failed to load session: {e}"),
            )
        })?;

        self.publish_session_ready(&session_id).await;
        self.send_available_commands_update(&args.session_id, &state.mcp_servers)
            .await;

        let current_mode = if state.mode.is_empty() {
            "default"
        } else {
            &state.mode
        };
        let current_model = state.model.as_deref().unwrap_or(&self.default_model);

        let allow_bypass = !is_running_as_root();
        Ok(ResumeSessionResponse::new()
            .modes(Self::build_mode_state(current_mode, allow_bypass))
            .models(Self::build_model_state(current_model))
            .config_options(Self::build_config_options(
                current_mode,
                current_model,
                allow_bypass,
            )))
    }

    async fn prompt(&self, args: PromptRequest) -> Result<PromptResponse> {
        agent_client_protocol::Agent::prompt(&self.bridge, args).await
    }

    async fn cancel(&self, args: CancelNotification) -> Result<()> {
        agent_client_protocol::Agent::cancel(&self.bridge, args).await
    }

    async fn ext_method(&self, args: ExtRequest) -> Result<ExtResponse> {
        // Handle session/close — not yet in agent-client-protocol 0.9.5
        if args.method.as_ref().contains("close") {
            let params: serde_json::Value =
                serde_json::from_str(args.params.get()).unwrap_or_default();
            if let Some(sid) = params.get("sessionId").and_then(|v| v.as_str()) {
                info!(session_id = %sid, "Close ACP session (ext_method)");
                self.close_session_impl(sid).await;
            }
            return Ok(ExtResponse::new(
                serde_json::value::RawValue::NULL.to_owned().into(),
            ));
        }
        Err(Error::new(
            ErrorCode::MethodNotFound.into(),
            format!("unknown ext method: {}", args.method),
        ))
    }

    async fn ext_notification(&self, _args: ExtNotification) -> Result<()> {
        Ok(())
    }
}

/// Convert a `TodoWrite` input JSON to ACP `PlanEntry` list for history replay.
fn replay_todo_write_to_plan(input: &serde_json::Value) -> Option<Vec<PlanEntry>> {
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

/// Sanitize a session title: collapse whitespace, trim, truncate to 256 chars.
fn sanitize_title(text: &str) -> String {
    let collapsed: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.chars().count() <= 256 {
        collapsed
    } else {
        let truncated: String = collapsed.chars().take(255).collect();
        format!("{truncated}…")
    }
}

/// Returns the current UTC time as an ISO-8601 string.
fn now_iso8601() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| {
            let secs = d.as_secs();
            let (y, mo, day, h, min, s) = epoch_to_parts(secs);
            format!("{y:04}-{mo:02}-{day:02}T{h:02}:{min:02}:{s:02}Z")
        })
        .unwrap_or_default()
}

fn epoch_to_parts(mut secs: u64) -> (u64, u64, u64, u64, u64, u64) {
    let s = secs % 60;
    secs /= 60;
    let min = secs % 60;
    secs /= 60;
    let h = secs % 24;
    secs /= 24;
    let mut days = secs;
    let mut year = 1970u64;
    loop {
        let dy = days_in_year(year);
        if days < dy {
            break;
        }
        days -= dy;
        year += 1;
    }
    let mut month = 1u64;
    loop {
        let dm = days_in_month(year, month);
        if days < dm {
            break;
        }
        days -= dm;
        month += 1;
    }
    (year, month, days + 1, h, min, s)
}

fn is_leap(y: u64) -> bool {
    (y.is_multiple_of(4) && !y.is_multiple_of(100)) || y.is_multiple_of(400)
}
fn days_in_year(y: u64) -> u64 {
    if is_leap(y) { 366 } else { 365 }
}
fn days_in_month(y: u64, m: u64) -> u64 {
    match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => {
            if is_leap(y) {
                29
            } else {
                28
            }
        }
        _ => 30,
    }
}

/// Returns `true` if the current process is running as root or under sudo.
fn is_running_as_root() -> bool {
    if std::env::var("SUDO_UID").is_ok() || std::env::var("SUDO_USER").is_ok() {
        return true;
    }
    #[cfg(target_os = "linux")]
    {
        if let Ok(status) = std::fs::read_to_string("/proc/self/status") {
            for line in status.lines() {
                if let Some(rest) = line.strip_prefix("Uid:\t")
                    && let Some(uid_str) = rest.split_whitespace().next()
                {
                    return uid_str == "0";
                }
            }
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    type TestAgent =
        TrogonAcpAgent<trogon_nats::AdvancedMockNatsClient, trogon_std::time::SystemClock>;

    // ── resolve_model ─────────────────────────────────────────────────────────

    #[test]
    fn resolve_model_exact_id_match() {
        assert_eq!(
            TestAgent::resolve_model("claude-opus-4-6"),
            Some("claude-opus-4-6")
        );
        assert_eq!(
            TestAgent::resolve_model("claude-sonnet-4-6"),
            Some("claude-sonnet-4-6")
        );
        assert_eq!(
            TestAgent::resolve_model("claude-haiku-4-5-20251001"),
            Some("claude-haiku-4-5-20251001")
        );
    }

    #[test]
    fn resolve_model_case_insensitive_name() {
        assert_eq!(
            TestAgent::resolve_model("claude opus 4"),
            Some("claude-opus-4-6")
        );
        assert_eq!(
            TestAgent::resolve_model("CLAUDE OPUS 4"),
            Some("claude-opus-4-6")
        );
    }

    #[test]
    fn resolve_model_substring_match() {
        assert_eq!(TestAgent::resolve_model("opus"), Some("claude-opus-4-6"));
        assert_eq!(
            TestAgent::resolve_model("sonnet"),
            Some("claude-sonnet-4-6")
        );
        assert_eq!(
            TestAgent::resolve_model("haiku"),
            Some("claude-haiku-4-5-20251001")
        );
    }

    #[test]
    fn resolve_model_tokenized_match() {
        assert_eq!(TestAgent::resolve_model("opus 4"), Some("claude-opus-4-6"));
    }

    #[test]
    fn resolve_model_empty_returns_none() {
        assert_eq!(TestAgent::resolve_model(""), None);
        assert_eq!(TestAgent::resolve_model("   "), None);
    }

    #[test]
    fn resolve_model_unknown_returns_none() {
        assert_eq!(TestAgent::resolve_model("gpt-4o"), None);
    }

    // ── sanitize_title ────────────────────────────────────────────────────────

    #[test]
    fn sanitize_title_collapses_whitespace() {
        assert_eq!(sanitize_title("  hello   world  "), "hello world");
    }

    #[test]
    fn sanitize_title_short_text_unchanged() {
        assert_eq!(sanitize_title("hello"), "hello");
    }

    #[test]
    fn sanitize_title_truncates_at_256_chars() {
        let long = "a".repeat(300);
        let out = sanitize_title(&long);
        assert!(out.ends_with('…'));
        assert_eq!(out.chars().count(), 256);
    }

    #[test]
    fn sanitize_title_unicode_multibyte_does_not_panic() {
        // "\u{1D56C}" is 4 bytes — 260 of them = 260 chars > 256, would panic on byte slice
        let s = "\u{1D56C}".repeat(260);
        let out = sanitize_title(&s);
        assert!(out.ends_with('…'));
        assert_eq!(out.chars().count(), 256);
    }

    #[test]
    fn sanitize_title_newlines_become_spaces() {
        let out = sanitize_title("line1\nline2\r\nline3");
        assert_eq!(out, "line1 line2 line3");
    }

    // ── epoch_to_parts ────────────────────────────────────────────────────────

    #[test]
    fn epoch_to_parts_unix_zero() {
        assert_eq!(epoch_to_parts(0), (1970, 1, 1, 0, 0, 0));
    }

    #[test]
    fn epoch_to_parts_known_date() {
        assert_eq!(epoch_to_parts(1_704_067_200), (2024, 1, 1, 0, 0, 0));
    }

    // ── is_leap ───────────────────────────────────────────────────────────────

    #[test]
    fn is_leap_2024_is_leap() {
        assert!(is_leap(2024));
    }
    #[test]
    fn is_leap_1900_is_not_leap() {
        assert!(!is_leap(1900));
    }
    #[test]
    fn is_leap_2000_is_leap() {
        assert!(is_leap(2000));
    }

    // ── is_running_as_root ────────────────────────────────────────────────────

    #[test]
    fn is_running_as_root_returns_bool_without_panic() {
        let _ = is_running_as_root();
    }

    // ── replay_todo_write_to_plan ─────────────────────────────────────────────

    #[test]
    fn replay_todo_write_to_plan_parses_three_entries() {
        let input = serde_json::json!({
            "todos": [
                { "content": "Write tests", "status": "in_progress", "priority": "high" },
                { "content": "Review PR", "status": "pending", "priority": "medium" },
                { "content": "Deploy", "status": "completed", "priority": "low" },
            ]
        });
        let entries = replay_todo_write_to_plan(&input).unwrap();
        assert_eq!(entries.len(), 3);
    }

    #[test]
    fn replay_todo_write_to_plan_status_in_progress() {
        let input = serde_json::json!({
            "todos": [{ "content": "task", "status": "in_progress", "priority": "high" }]
        });
        let entries = replay_todo_write_to_plan(&input).unwrap();
        assert!(matches!(entries[0].status, PlanEntryStatus::InProgress));
    }

    #[test]
    fn replay_todo_write_to_plan_status_completed() {
        let input = serde_json::json!({
            "todos": [{ "content": "task", "status": "completed", "priority": "high" }]
        });
        let entries = replay_todo_write_to_plan(&input).unwrap();
        assert!(matches!(entries[0].status, PlanEntryStatus::Completed));
    }

    #[test]
    fn replay_todo_write_to_plan_priority_medium() {
        let input = serde_json::json!({
            "todos": [{ "content": "task", "status": "pending", "priority": "medium" }]
        });
        let entries = replay_todo_write_to_plan(&input).unwrap();
        assert!(matches!(entries[0].priority, PlanEntryPriority::Medium));
    }

    #[test]
    fn replay_todo_write_to_plan_priority_low() {
        let input = serde_json::json!({
            "todos": [{ "content": "task", "status": "pending", "priority": "low" }]
        });
        let entries = replay_todo_write_to_plan(&input).unwrap();
        assert!(matches!(entries[0].priority, PlanEntryPriority::Low));
    }

    #[test]
    fn replay_todo_write_to_plan_returns_none_for_empty_todos() {
        let input = serde_json::json!({ "todos": [] });
        assert!(replay_todo_write_to_plan(&input).is_none());
    }

    #[test]
    fn replay_todo_write_to_plan_returns_none_when_no_todos_key() {
        let input = serde_json::json!({ "other": "value" });
        assert!(replay_todo_write_to_plan(&input).is_none());
    }

    // ── build_mode_state ──────────────────────────────────────────────────────

    #[test]
    fn build_mode_state_without_bypass_has_4_modes() {
        let state = TestAgent::build_mode_state("default", false);
        assert_eq!(state.available_modes.len(), 4);
        assert_eq!(state.current_mode_id.to_string(), "default");
    }

    #[test]
    fn build_mode_state_with_bypass_has_5_modes() {
        let state = TestAgent::build_mode_state("plan", true);
        assert_eq!(state.available_modes.len(), 5);
        let ids: Vec<String> = state
            .available_modes
            .iter()
            .map(|m| m.id.to_string())
            .collect();
        assert!(ids.iter().any(|id| id == "bypassPermissions"));
    }

    // ── build_model_state ─────────────────────────────────────────────────────

    #[test]
    fn build_model_state_contains_all_known_models() {
        let state = TestAgent::build_model_state("claude-sonnet-4-6");
        assert_eq!(state.current_model_id.to_string(), "claude-sonnet-4-6");
        assert_eq!(state.available_models.len(), 3);
        let ids: Vec<String> = state
            .available_models
            .iter()
            .map(|m| m.model_id.to_string())
            .collect();
        assert!(ids.iter().any(|id| id == "claude-opus-4-6"));
        assert!(ids.iter().any(|id| id == "claude-sonnet-4-6"));
        assert!(ids.iter().any(|id| id == "claude-haiku-4-5-20251001"));
    }

    // ── build_config_options ──────────────────────────────────────────────────

    #[test]
    fn build_config_options_returns_mode_and_model() {
        let opts = TestAgent::build_config_options("default", "claude-sonnet-4-6", false);
        assert_eq!(opts.len(), 2);
        let ids: Vec<String> = opts.iter().map(|o| o.id.to_string()).collect();
        assert!(ids.iter().any(|id| id == "mode"));
        assert!(ids.iter().any(|id| id == "model"));
    }

    // ── convert_mcp_servers ───────────────────────────────────────────────────

    #[test]
    fn convert_mcp_servers_http_server_included() {
        use agent_client_protocol::McpServerHttp;
        let servers = vec![McpServer::Http(McpServerHttp::new(
            "myserver",
            "http://localhost:8080",
        ))];
        let stored = TestAgent::convert_mcp_servers(&servers);
        assert_eq!(stored.len(), 1);
        assert_eq!(stored[0].name, "myserver");
        assert_eq!(stored[0].url, "http://localhost:8080");
    }

    #[test]
    fn convert_mcp_servers_stdio_excluded() {
        use agent_client_protocol::McpServerStdio;
        let servers = vec![McpServer::Stdio(McpServerStdio::new("local", "npx"))];
        let stored = TestAgent::convert_mcp_servers(&servers);
        assert!(stored.is_empty(), "Stdio servers must be filtered out");
    }

    #[test]
    fn convert_mcp_servers_empty_input() {
        let stored = TestAgent::convert_mcp_servers(&[]);
        assert!(stored.is_empty());
    }

    // ── Integration tests (require Docker) ────────────────────────────────────
    //
    // These tests spin up a real NATS server via testcontainers and exercise
    // the full TrogonAcpAgent lifecycle against a live JetStream KV bucket.

    mod integration {
        use super::super::*;
        use acp_nats::{AcpPrefix, Bridge, Config, NatsAuth, NatsConfig};
        use agent_client_protocol::{
            Agent, AuthenticateRequest, ExtRequest, ForkSessionRequest, InitializeRequest,
            ListSessionsRequest, LoadSessionRequest, NewSessionRequest, ResumeSessionRequest,
            SessionId, SetSessionConfigOptionRequest, SetSessionModeRequest,
            SetSessionModelRequest,
        };
        use async_nats::jetstream;
        use std::sync::Arc;
        use testcontainers_modules::nats::Nats;
        use testcontainers_modules::testcontainers::runners::AsyncRunner;
        use testcontainers_modules::testcontainers::{ContainerAsync, ImageExt};
        use tokio::sync::{RwLock, mpsc};
        use trogon_acp_runner::{GatewayConfig, SessionState, SessionStore};
        use trogon_std::time::SystemClock;

        type RealAgent = TrogonAcpAgent<async_nats::Client, SystemClock>;

        async fn start_nats() -> (ContainerAsync<Nats>, async_nats::Client, jetstream::Context) {
            let container: ContainerAsync<Nats> = Nats::default()
                .with_cmd(["--jetstream"])
                .start()
                .await
                .expect("Docker must be running for integration tests");
            let port = container.get_host_port_ipv4(4222).await.unwrap();
            let nats = async_nats::connect(format!("127.0.0.1:{port}"))
                .await
                .expect("failed to connect to NATS");
            let js = jetstream::new(nats.clone());
            (container, nats, js)
        }

        async fn make_agent(
            nats: async_nats::Client,
            js: &jetstream::Context,
        ) -> (
            RealAgent,
            tokio::sync::mpsc::Receiver<agent_client_protocol::SessionNotification>,
        ) {
            let store = SessionStore::open(js).await.unwrap();
            let (notif_tx, notif_rx) = mpsc::channel(64);
            let gateway_config = Arc::new(RwLock::new(None::<GatewayConfig>));

            let config = Config::new(
                AcpPrefix::new("acp").unwrap(),
                NatsConfig {
                    servers: vec!["unused".into()],
                    auth: NatsAuth::None,
                },
            );
            let bridge = Bridge::new(
                nats.clone(),
                SystemClock,
                &opentelemetry::global::meter("acp-test"),
                config,
                notif_tx.clone(),
            );

            let agent = TrogonAcpAgent::new(
                bridge,
                store,
                nats,
                "acp",
                notif_tx,
                "claude-opus-4-6",
                gateway_config,
            );
            (agent, notif_rx)
        }

        // ── initialize ────────────────────────────────────────────────────────

        #[tokio::test(flavor = "current_thread")]
        async fn initialize_returns_protocol_version() {
            let (_c, nats, js) = start_nats().await;
            let (agent, _rx) = make_agent(nats, &js).await;

            let req = InitializeRequest::new(agent_client_protocol::ProtocolVersion::LATEST);
            let resp = agent.initialize(req).await.unwrap();
            assert_eq!(
                resp.protocol_version,
                agent_client_protocol::ProtocolVersion::LATEST
            );
        }

        #[tokio::test(flavor = "current_thread")]
        async fn initialize_advertises_load_session_capability() {
            let (_c, nats, js) = start_nats().await;
            let (agent, _rx) = make_agent(nats, &js).await;

            let req = InitializeRequest::new(agent_client_protocol::ProtocolVersion::LATEST);
            let resp = agent.initialize(req).await.unwrap();
            let caps = resp.agent_capabilities;
            assert!(caps.load_session, "must advertise load_session capability");
        }

        // ── authenticate ──────────────────────────────────────────────────────

        #[tokio::test(flavor = "current_thread")]
        async fn authenticate_unsupported_method_returns_error() {
            let (_c, nats, js) = start_nats().await;
            let (agent, _rx) = make_agent(nats, &js).await;

            let req = AuthenticateRequest::new("oauth");
            let err = agent.authenticate(req).await.unwrap_err();
            assert!(err.to_string().contains("unsupported auth method"));
        }

        #[tokio::test(flavor = "current_thread")]
        async fn authenticate_gateway_stores_config() {
            let (_c, nats, js) = start_nats().await;
            let (agent, _rx) = make_agent(nats, &js).await;

            let meta = serde_json::json!({
                "gateway": {
                    "baseUrl": "https://gateway.example.com/v1",
                    "headers": { "Authorization": "Bearer tok-abc123" }
                }
            });
            let req = AuthenticateRequest::new("gateway").meta(
                serde_json::from_value::<serde_json::Map<String, serde_json::Value>>(meta).unwrap(),
            );
            agent.authenticate(req).await.unwrap();

            let cfg = agent.gateway_config.read().await;
            let gw = cfg.as_ref().expect("gateway config should be stored");
            assert_eq!(gw.base_url, "https://gateway.example.com/v1");
            assert_eq!(gw.token, "tok-abc123");
        }

        // ── new_session ───────────────────────────────────────────────────────

        #[tokio::test(flavor = "current_thread")]
        async fn new_session_returns_session_id() {
            let (_c, nats, js) = start_nats().await;
            let (agent, _rx) = make_agent(nats, &js).await;

            let req = NewSessionRequest::new("/home/user/project");
            let resp = agent.new_session(req).await.unwrap();
            let sid = resp.session_id.to_string();
            assert!(!sid.is_empty(), "session_id must be set");
        }

        #[tokio::test(flavor = "current_thread")]
        async fn new_session_persists_state_in_kv() {
            let (_c, nats, js) = start_nats().await;
            let (agent, _rx) = make_agent(nats.clone(), &js).await;

            let req = NewSessionRequest::new("/workspace/myproject");
            let resp = agent.new_session(req).await.unwrap();
            let sid = resp.session_id.to_string();

            // Read back from the same KV bucket
            let store2 = SessionStore::open(&js).await.unwrap();
            let state = store2.load(&sid).await.unwrap();
            assert_eq!(state.cwd, "/workspace/myproject");
            assert_eq!(state.mode, "default");
        }

        #[tokio::test(flavor = "current_thread")]
        async fn new_session_returns_mode_and_model_state() {
            let (_c, nats, js) = start_nats().await;
            let (agent, _rx) = make_agent(nats, &js).await;

            let req = NewSessionRequest::new("/tmp");
            let resp = agent.new_session(req).await.unwrap();
            assert!(resp.modes.is_some(), "modes must be present");
            assert!(resp.models.is_some(), "models must be present");
        }

        // ── load_session ──────────────────────────────────────────────────────

        #[tokio::test(flavor = "current_thread")]
        async fn load_session_succeeds_for_existing_session() {
            let (_c, nats, js) = start_nats().await;
            let (agent, _rx) = make_agent(nats, &js).await;

            // Create session first
            let new_resp = agent
                .new_session(NewSessionRequest::new("/tmp"))
                .await
                .unwrap();
            let sid = new_resp.session_id;

            let load_req = LoadSessionRequest::new(sid.clone(), "/tmp");
            let load_resp = agent.load_session(load_req).await.unwrap();
            assert!(load_resp.modes.is_some(), "modes must be returned on load");
        }

        #[tokio::test(flavor = "current_thread")]
        async fn load_session_missing_session_returns_empty_state() {
            let (_c, nats, js) = start_nats().await;
            let (agent, _rx) = make_agent(nats, &js).await;

            // Loading a non-existent session returns default (empty) state without error
            let load_req = LoadSessionRequest::new(SessionId::from("no-such-session"), "/tmp");
            let result = agent.load_session(load_req).await;
            assert!(
                result.is_ok(),
                "load of missing session should succeed (returns empty default)"
            );
        }

        // ── set_session_mode ──────────────────────────────────────────────────

        #[tokio::test(flavor = "current_thread")]
        async fn set_session_mode_valid_mode_persists() {
            let (_c, nats, js) = start_nats().await;
            let (agent, _rx) = make_agent(nats, &js).await;

            let new_resp = agent
                .new_session(NewSessionRequest::new("/tmp"))
                .await
                .unwrap();
            let sid = new_resp.session_id.clone();

            let req = SetSessionModeRequest::new(sid.clone(), "acceptEdits");
            agent.set_session_mode(req).await.unwrap();

            let store = SessionStore::open(&js).await.unwrap();
            let state = store.load(&sid.to_string()).await.unwrap();
            assert_eq!(state.mode, "acceptEdits");
        }

        #[tokio::test(flavor = "current_thread")]
        async fn set_session_mode_invalid_mode_returns_error() {
            let (_c, nats, js) = start_nats().await;
            let (agent, _rx) = make_agent(nats, &js).await;

            let new_resp = agent
                .new_session(NewSessionRequest::new("/tmp"))
                .await
                .unwrap();
            let sid = new_resp.session_id;

            let req = SetSessionModeRequest::new(sid, "invalidMode");
            let err = agent.set_session_mode(req).await.unwrap_err();
            assert!(err.to_string().contains("Invalid mode"));
        }

        // ── set_session_config_option ──────────────────────────────────────────

        #[tokio::test(flavor = "current_thread")]
        async fn set_session_config_option_mode_persists() {
            let (_c, nats, js) = start_nats().await;
            let (agent, _rx) = make_agent(nats, &js).await;

            let new_resp = agent
                .new_session(NewSessionRequest::new("/tmp"))
                .await
                .unwrap();
            let sid = new_resp.session_id;

            let req = SetSessionConfigOptionRequest::new(sid.clone(), "mode", "plan");
            agent.set_session_config_option(req).await.unwrap();

            let store = SessionStore::open(&js).await.unwrap();
            let state = store.load(&sid.to_string()).await.unwrap();
            assert_eq!(state.mode, "plan");
        }

        #[tokio::test(flavor = "current_thread")]
        async fn set_session_config_option_mode_sends_current_mode_update() {
            let (_c, nats, js) = start_nats().await;
            let (agent, mut rx) = make_agent(nats, &js).await;

            let new_resp = agent
                .new_session(NewSessionRequest::new("/tmp"))
                .await
                .unwrap();
            let sid = new_resp.session_id;
            // drain notifications from new_session (including any async-spawned ones)
            while rx.try_recv().is_ok() {}

            let req = SetSessionConfigOptionRequest::new(sid, "mode", "acceptEdits");
            agent.set_session_config_option(req).await.unwrap();

            // Scan notifications — AvailableCommandsUpdate from new_session may also appear
            let mut found = false;
            while let Ok(notif) = rx.try_recv() {
                if matches!(notif.update, SessionUpdate::CurrentModeUpdate(_)) {
                    found = true;
                    break;
                }
            }
            assert!(found, "expected CurrentModeUpdate notification");
        }

        #[tokio::test(flavor = "current_thread")]
        async fn set_session_config_option_invalid_mode_returns_error() {
            let (_c, nats, js) = start_nats().await;
            let (agent, _rx) = make_agent(nats, &js).await;

            let new_resp = agent
                .new_session(NewSessionRequest::new("/tmp"))
                .await
                .unwrap();
            let sid = new_resp.session_id;

            let req = SetSessionConfigOptionRequest::new(sid, "mode", "invalidMode");
            let err = agent.set_session_config_option(req).await.unwrap_err();
            assert!(err.to_string().contains("Invalid mode"));
        }

        #[tokio::test(flavor = "current_thread")]
        async fn set_session_config_option_model_persists() {
            let (_c, nats, js) = start_nats().await;
            let (agent, _rx) = make_agent(nats, &js).await;

            let new_resp = agent
                .new_session(NewSessionRequest::new("/tmp"))
                .await
                .unwrap();
            let sid = new_resp.session_id;

            let req = SetSessionConfigOptionRequest::new(sid.clone(), "model", "claude-opus-4-6");
            agent.set_session_config_option(req).await.unwrap();

            let store = SessionStore::open(&js).await.unwrap();
            let state = store.load(&sid.to_string()).await.unwrap();
            assert_eq!(state.model.as_deref(), Some("claude-opus-4-6"));
        }

        // ── set_session_model ──────────────────────────────────────────────────

        #[tokio::test(flavor = "current_thread")]
        async fn set_session_model_fuzzy_persists() {
            let (_c, nats, js) = start_nats().await;
            let (agent, _rx) = make_agent(nats, &js).await;

            let new_resp = agent
                .new_session(NewSessionRequest::new("/tmp"))
                .await
                .unwrap();
            let sid = new_resp.session_id;

            let req = SetSessionModelRequest::new(sid.clone(), "sonnet");
            agent.set_session_model(req).await.unwrap();

            let store = SessionStore::open(&js).await.unwrap();
            let state = store.load(&sid.to_string()).await.unwrap();
            assert_eq!(state.model.as_deref(), Some("claude-sonnet-4-6"));
        }

        // ── list_sessions ──────────────────────────────────────────────────────

        #[tokio::test(flavor = "current_thread")]
        async fn list_sessions_returns_all() {
            let (_c, nats, js) = start_nats().await;
            let (agent, _rx) = make_agent(nats, &js).await;

            agent
                .new_session(NewSessionRequest::new("/workspace/a"))
                .await
                .unwrap();
            agent
                .new_session(NewSessionRequest::new("/workspace/b"))
                .await
                .unwrap();

            let resp = agent
                .list_sessions(ListSessionsRequest::new())
                .await
                .unwrap();
            assert_eq!(resp.sessions.len(), 2);
        }

        #[tokio::test(flavor = "current_thread")]
        async fn list_sessions_cwd_filter() {
            let (_c, nats, js) = start_nats().await;
            let (agent, _rx) = make_agent(nats, &js).await;

            agent
                .new_session(NewSessionRequest::new("/project/api"))
                .await
                .unwrap();
            agent
                .new_session(NewSessionRequest::new("/other/service"))
                .await
                .unwrap();

            let req = ListSessionsRequest::new().cwd(Some(std::path::PathBuf::from("/project")));
            let resp = agent.list_sessions(req).await.unwrap();
            assert_eq!(resp.sessions.len(), 1);
            assert_eq!(
                resp.sessions[0].cwd,
                std::path::PathBuf::from("/project/api")
            );
        }

        #[tokio::test(flavor = "current_thread")]
        async fn list_sessions_skips_empty_cwd() {
            let (_c, nats, js) = start_nats().await;
            let (agent, _rx) = make_agent(nats, &js).await;

            // Manually save a session with empty cwd
            let store = SessionStore::open(&js).await.unwrap();
            store
                .save(
                    "no-cwd",
                    &SessionState {
                        cwd: String::new(),
                        mode: "default".to_string(),
                        ..Default::default()
                    },
                )
                .await
                .unwrap();

            // Also create a normal session
            agent
                .new_session(NewSessionRequest::new("/real/path"))
                .await
                .unwrap();

            let resp = agent
                .list_sessions(ListSessionsRequest::new())
                .await
                .unwrap();
            // Only the session with a real cwd should appear
            assert_eq!(resp.sessions.len(), 1);
            assert_eq!(resp.sessions[0].cwd, std::path::PathBuf::from("/real/path"));
        }

        // ── fork_session ───────────────────────────────────────────────────────

        #[tokio::test(flavor = "current_thread")]
        async fn fork_session_preserves_mode_and_model() {
            let (_c, nats, js) = start_nats().await;
            let (agent, _rx) = make_agent(nats, &js).await;

            let new_resp = agent
                .new_session(NewSessionRequest::new("/src"))
                .await
                .unwrap();
            let src_id = new_resp.session_id.clone();

            // Patch source session's mode and model via store
            let store = SessionStore::open(&js).await.unwrap();
            let mut state = store.load(&src_id.to_string()).await.unwrap();
            state.mode = "plan".to_string();
            state.model = Some("claude-opus-4-6".to_string());
            store.save(&src_id.to_string(), &state).await.unwrap();

            let fork_req = ForkSessionRequest::new(src_id, "/forked");
            let fork_resp = agent.fork_session(fork_req).await.unwrap();
            let forked_id = fork_resp.session_id.to_string();

            let forked_state = store.load(&forked_id).await.unwrap();
            assert_eq!(forked_state.mode, "plan");
            assert_eq!(forked_state.model.as_deref(), Some("claude-opus-4-6"));
            assert_eq!(forked_state.cwd, "/forked");
        }

        #[tokio::test(flavor = "current_thread")]
        async fn fork_session_returns_new_session_id() {
            let (_c, nats, js) = start_nats().await;
            let (agent, _rx) = make_agent(nats, &js).await;

            let new_resp = agent
                .new_session(NewSessionRequest::new("/src"))
                .await
                .unwrap();
            let src_id = new_resp.session_id.clone();

            let fork_resp = agent
                .fork_session(ForkSessionRequest::new(src_id.clone(), "/dest"))
                .await
                .unwrap();
            assert_ne!(
                fork_resp.session_id.to_string(),
                src_id.to_string(),
                "fork must produce a new session ID"
            );
        }

        // ── resume_session ─────────────────────────────────────────────────────

        #[tokio::test(flavor = "current_thread")]
        async fn resume_session_returns_modes_and_models() {
            let (_c, nats, js) = start_nats().await;
            let (agent, _rx) = make_agent(nats, &js).await;

            let new_resp = agent
                .new_session(NewSessionRequest::new("/tmp"))
                .await
                .unwrap();
            let sid = new_resp.session_id;

            let req = ResumeSessionRequest::new(sid, "/tmp");
            let resp = agent.resume_session(req).await.unwrap();
            assert!(resp.modes.is_some(), "modes must be present on resume");
            assert!(resp.models.is_some(), "models must be present on resume");
        }

        // ── ext_method ─────────────────────────────────────────────────────────

        #[tokio::test(flavor = "current_thread")]
        async fn ext_method_close_deletes_session() {
            let (_c, nats, js) = start_nats().await;
            let (agent, _rx) = make_agent(nats, &js).await;

            let new_resp = agent
                .new_session(NewSessionRequest::new("/tmp"))
                .await
                .unwrap();
            let sid = new_resp.session_id.to_string();

            let params_json = format!(r#"{{"sessionId":"{}"}}"#, sid);
            let params: Arc<serde_json::value::RawValue> =
                serde_json::value::RawValue::from_string(params_json)
                    .unwrap()
                    .into();
            agent
                .ext_method(ExtRequest::new("session/close", params))
                .await
                .unwrap();

            let store = SessionStore::open(&js).await.unwrap();
            let state = store.load(&sid).await.unwrap();
            assert_eq!(state.cwd, "", "deleted session must return empty default");
        }

        #[tokio::test(flavor = "current_thread")]
        async fn ext_method_unknown_returns_method_not_found() {
            let (_c, nats, js) = start_nats().await;
            let (agent, _rx) = make_agent(nats, &js).await;

            let params: Arc<serde_json::value::RawValue> =
                serde_json::value::RawValue::from_string("{}".to_string())
                    .unwrap()
                    .into();
            let err = agent
                .ext_method(ExtRequest::new("session/unknown_action", params))
                .await
                .unwrap_err();
            assert!(err.to_string().contains("unknown ext method"));
        }

        // ── replay_history ─────────────────────────────────────────────────────

        #[tokio::test(flavor = "current_thread")]
        async fn replay_history_text_sends_agent_message_chunk() {
            use trogon_agent_core::agent_loop::{ContentBlock as AgentCb, Message as AgentMsg};
            let (_c, nats, js) = start_nats().await;
            let (agent, mut rx) = make_agent(nats, &js).await;

            let state = SessionState {
                messages: vec![AgentMsg {
                    role: "assistant".to_string(),
                    content: vec![AgentCb::Text {
                        text: "hello world".to_string(),
                    }],
                }],
                ..Default::default()
            };
            agent
                .replay_history(&SessionId::from("replay-text"), &state)
                .await;

            let notif = rx.try_recv().expect("expected notification");
            assert!(matches!(notif.update, SessionUpdate::AgentMessageChunk(_)));
        }

        #[tokio::test(flavor = "current_thread")]
        async fn replay_history_thinking_sends_thought_chunk() {
            use trogon_agent_core::agent_loop::{ContentBlock as AgentCb, Message as AgentMsg};
            let (_c, nats, js) = start_nats().await;
            let (agent, mut rx) = make_agent(nats, &js).await;

            let state = SessionState {
                messages: vec![AgentMsg {
                    role: "assistant".to_string(),
                    content: vec![AgentCb::Thinking {
                        thinking: "I'm thinking...".to_string(),
                    }],
                }],
                ..Default::default()
            };
            agent
                .replay_history(&SessionId::from("replay-think"), &state)
                .await;

            let notif = rx.try_recv().expect("expected notification");
            assert!(matches!(notif.update, SessionUpdate::AgentThoughtChunk(_)));
        }

        #[tokio::test(flavor = "current_thread")]
        async fn replay_history_tool_use_sends_tool_call() {
            use trogon_agent_core::agent_loop::{ContentBlock as AgentCb, Message as AgentMsg};
            let (_c, nats, js) = start_nats().await;
            let (agent, mut rx) = make_agent(nats, &js).await;

            let state = SessionState {
                messages: vec![AgentMsg {
                    role: "assistant".to_string(),
                    content: vec![AgentCb::ToolUse {
                        id: "tu-1".to_string(),
                        name: "Bash".to_string(),
                        input: serde_json::json!({"command": "ls"}),
                    }],
                }],
                ..Default::default()
            };
            agent
                .replay_history(&SessionId::from("replay-tool"), &state)
                .await;

            let notif = rx.try_recv().expect("expected notification");
            assert!(matches!(notif.update, SessionUpdate::ToolCall(_)));
        }

        #[tokio::test(flavor = "current_thread")]
        async fn replay_history_tool_result_sends_tool_call_update() {
            use trogon_agent_core::agent_loop::{ContentBlock as AgentCb, Message as AgentMsg};
            let (_c, nats, js) = start_nats().await;
            let (agent, mut rx) = make_agent(nats, &js).await;

            let state = SessionState {
                messages: vec![
                    AgentMsg {
                        role: "assistant".to_string(),
                        content: vec![AgentCb::ToolUse {
                            id: "tu-1".to_string(),
                            name: "Bash".to_string(),
                            input: serde_json::json!({"command": "ls"}),
                        }],
                    },
                    AgentMsg {
                        role: "user".to_string(),
                        content: vec![AgentCb::ToolResult {
                            tool_use_id: "tu-1".to_string(),
                            content: "file1.txt\nfile2.txt".to_string(),
                        }],
                    },
                ],
                ..Default::default()
            };
            agent
                .replay_history(&SessionId::from("replay-result"), &state)
                .await;

            // First: ToolCall from the assistant tool_use block
            let notif1 = rx.try_recv().expect("expected ToolCall notification");
            assert!(matches!(notif1.update, SessionUpdate::ToolCall(_)));
            // Second: ToolCallUpdate from the user tool_result block
            let notif2 = rx.try_recv().expect("expected ToolCallUpdate notification");
            assert!(matches!(notif2.update, SessionUpdate::ToolCallUpdate(_)));
        }

        #[tokio::test(flavor = "current_thread")]
        async fn replay_history_todo_write_sends_plan() {
            use trogon_agent_core::agent_loop::{ContentBlock as AgentCb, Message as AgentMsg};
            let (_c, nats, js) = start_nats().await;
            let (agent, mut rx) = make_agent(nats, &js).await;

            let state = SessionState {
                messages: vec![AgentMsg {
                    role: "assistant".to_string(),
                    content: vec![AgentCb::ToolUse {
                        id: "tw-1".to_string(),
                        name: "TodoWrite".to_string(),
                        input: serde_json::json!({
                            "todos": [
                                { "content": "Write tests", "status": "in_progress", "priority": "high" }
                            ]
                        }),
                    }],
                }],
                ..Default::default()
            };
            agent
                .replay_history(&SessionId::from("replay-todo"), &state)
                .await;

            let notif = rx.try_recv().expect("expected Plan notification");
            assert!(matches!(notif.update, SessionUpdate::Plan(_)));
        }
    }
}

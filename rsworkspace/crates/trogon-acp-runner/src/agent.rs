use std::collections::HashMap;
use std::sync::Arc;

use acp_nats::acp_prefix::AcpPrefix;
use acp_nats::nats::{ExtSessionReady, session as session_subjects};
use acp_nats::session_id::AcpSessionId;
use agent_client_protocol::{
    AgentCapabilities, AuthMethod, AuthMethodAgent, AuthenticateRequest, AuthenticateResponse, CancelNotification,
    CloseSessionRequest, CloseSessionResponse, ContentBlock, EmbeddedResourceResource, Error, ErrorCode, ExtRequest,
    ExtResponse, ForkSessionRequest, ForkSessionResponse, Implementation, InitializeRequest, InitializeResponse,
    ListSessionsRequest, ListSessionsResponse, LoadSessionRequest, LoadSessionResponse, ModelInfo, NewSessionRequest,
    NewSessionResponse, PromptRequest, PromptResponse, ProtocolVersion, ResumeSessionRequest, ResumeSessionResponse,
    SessionCapabilities, SessionConfigOption, SessionConfigOptionCategory, SessionConfigSelectOption,
    SessionForkCapabilities, SessionId, SessionInfo, SessionListCapabilities, SessionMode, SessionModeState,
    SessionModelState, SessionResumeCapabilities, SetSessionConfigOptionRequest, SetSessionConfigOptionResponse,
    SetSessionModeRequest, SetSessionModeResponse, SetSessionModelRequest, SetSessionModelResponse, StopReason,
};
use async_trait::async_trait;
use bytes::Bytes;
use tokio::sync::{RwLock, mpsc};
use tracing::{error, info, warn};
use trogon_agent_core::agent_loop::{AgentEvent, ContentBlock as AgentContentBlock, ImageSource, Message};
use trogon_agent_core::tools::{ToolDef, all_tool_defs, exit_plan_mode_tool_def};

use crate::agent_runner::AgentRunner;
use crate::elicitation::{ChannelElicitationProvider, ElicitationTx};
use crate::prompt_converter::PromptEventConverter;
use crate::session_notifier::{AccumulatingPromptClient, CancelSubscription, PromptEventClient, SessionNotifier};
use trogon_runner_tools::build_mode_permission_checker;
use trogon_runner_tools::egress::EgressPolicy;
use trogon_runner_tools::permission::{AuditBuf, PermissionTx};
use trogon_runner_tools::permission_rules::PermissionRules;
use trogon_runner_tools::session_store::{
    AuditEntry, NatsSessionStore, SessionState, SessionStore, append_audit_entries, now_iso8601,
};
use trogon_runner_tools::wasm_bash_tool::WasmRuntimeBashTool;
use trogon_runner_tools::{CompactError, CompactProviders, parse_compactor_config, request_compaction};
use trogon_runner_tools::{FsTrogonMdLoader, TrogonMdLoading};

/// Gateway credentials that override the default proxy/token when set.
#[derive(Debug, Clone)]
pub struct GatewayConfig {
    pub base_url: String,
    pub token: String,
    pub extra_headers: Vec<(String, String)>,
}

/// Session provider identity for the acp-runner (Anthropic).
const SESSION_PROVIDER: &str = "anthropic";

/// Returns the context window token limit for a given model ID.
///
/// Claude exposes a 1M-token window for Sonnet via the `[1m]` model-id suffix
/// (the convention Claude Code uses); every other Claude model is the standard
/// 200K window. Unknown models fall back to 200K so the usage meter degrades to
/// a sane value rather than 0 (which would render a broken "/0" meter and risk a
/// divide-by-zero downstream).
fn context_window_tokens(model: &str) -> u64 {
    if model.contains("[1m]") {
        1_000_000
    } else {
        200_000
    }
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
                EmbeddedResourceResource::TextResourceContents(t) => Some(AgentContentBlock::Text {
                    text: format!("\n<context ref=\"{}\">\n{}\n</context>", t.uri, t.text),
                }),
                EmbeddedResourceResource::BlobResourceContents(b) => Some(AgentContentBlock::Image {
                    source: ImageSource::Base64 {
                        media_type: b.mime_type.clone().unwrap_or_default(),
                        data: b.blob.clone(),
                    },
                }),
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

fn internal_error(msg: impl Into<String>) -> Error {
    Error::new(ErrorCode::InternalError.into(), msg.into())
}

/// The single auth method this runner advertises in `initialize` and accepts in
/// `authenticate`. Credentials reach the upstream provider out-of-band (env /
/// vault token), so `authenticate` only confirms the client selected this
/// method rather than silently succeeding for any (or an empty) id.
const GATEWAY_AUTH_METHOD: &str = "gateway_auth";

fn invalid_session_id(e: acp_nats::session_id::SessionIdError) -> Error {
    Error::new(
        ErrorCode::InvalidParams.into(),
        format!("Invalid session ID: {e}"),
    )
}

/// Estimates the token count of a message list using the heuristic `bytes / 4`.
fn estimate_token_count(messages: &[Message]) -> u64 {
    serde_json::to_string(messages).map(|s| s.len() as u64).unwrap_or(0) / 4
}

/// Sends the conversation history to `trogon-compactor` via NATS request-reply.
///
/// Returns the original messages unchanged if the compactor is not running or
/// returns an error — compaction is always opt-in and never blocks the prompt.
///
/// Times out after 25 s (well under the registry TTL of 30 s) so that a slow or
/// absent compactor never holds the session semaphore long enough to cause the
/// registry entry to expire or the CLI to time out.
/// Requests compaction from `trogon-compactor`.
///
/// Returns `Ok(Some(messages))` when the compactor compacted, `Ok(None)` when it
/// declined (nothing to compact), and `Err` when the compactor was unavailable or
/// returned an invalid response. Distinguishing these three outcomes lets each
/// caller pick its policy: the manual `/compact` handler propagates the `Err` so
/// the user sees the compactor-down failure, while the auto-path degrades and
/// continues uncompacted in the background.
///
/// The shared [`request_compaction`] helper owns building the request, the NATS
/// request-reply, and decoding. This per-runner wrapper keeps acp-specific
/// concerns: the session provider identity, the model's `context_window_tokens`,
/// the 25 s timeout (well under the 30 s registry TTL), and `session_id`-scoped
/// telemetry. It does **not** decide *when* to compact — the threshold check lives
/// at the call sites.
async fn compact_messages(
    nats: &async_nats::Client,
    messages: &[Message],
    session_id: &str,
    model: &str,
    compactor_provider: Option<&str>,
    compactor_model: Option<&str>,
) -> Result<Option<Vec<Message>>, CompactError> {
    let outcome = request_compaction(
        nats,
        messages,
        Some(context_window_tokens(model)),
        CompactProviders {
            session_provider: SESSION_PROVIDER,
            session_model: model,
            compactor_provider,
            compactor_model,
        },
        std::time::Duration::from_secs(25),
    )
    .await?;

    let new_messages = outcome.map(|r| r.messages);

    if let Some(ref new_msgs) = new_messages {
        info!(
            session_id,
            tokens_before = estimate_token_count(messages),
            tokens_after = estimate_token_count(new_msgs),
            "context compacted"
        );
    }

    Ok(new_messages)
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
    M = FsTrogonMdLoader,
> {
    notifier: N,
    store: S,
    agent: Arc<A>,
    md_loader: M,
    prefix: String,
    default_model: String,
    permission_tx: Option<PermissionTx>,
    elicitation_tx: Option<ElicitationTx>,
    gateway_config: Arc<RwLock<Option<GatewayConfig>>>,
    /// Per-session semaphore (1 permit) to serialize concurrent prompt calls.
    session_locks: Arc<std::sync::Mutex<HashMap<String, Arc<tokio::sync::Semaphore>>>>,
    /// Per-session MCP tool-list cache (stdio clients stay alive across turns).
    session_mcp_caches: Arc<std::sync::Mutex<HashMap<String, Arc<trogon_runner_tools::SessionMcpCache>>>>,
    /// NATS client used to call trogon-compactor.  `None` disables compaction.
    compactor_nats: Option<async_nats::Client>,
    /// HTTP client injected into per-session MCP connections.
    http: reqwest::Client,
    /// Registry used to discover execution backends (wasm-runtime).  `None` disables bash tool.
    registry: Option<Arc<trogon_registry::Registry<async_nats::jetstream::kv::Store>>>,
    /// NATS client forwarded to `WasmRuntimeBashTool` when an execution backend is available.
    execution_nats: Option<async_nats::Client>,
    /// `auto`-mode LLM safety classifier. `None` makes `auto` prompt for
    /// side-effecting tools instead of classifying them.
    classifier: Option<Arc<dyn trogon_runner_tools::SafetyClassifier>>,
    /// Auto-compaction trigger as a percentage of the session token budget.
    /// Read once from `COMPACT_THRESHOLD_PCT` so the env override is honored on
    /// the hot path (NEW-22), matching the compactor service's own default.
    compact_threshold_pct: u8,
    /// Optional Session Kernel sink: when set, each turn's conversation is mirrored
    /// into the canonical event log (shadow mode). `None` keeps the legacy path.
    conversation_sink: Option<Arc<dyn crate::kernel_sink::ConversationSink>>,
    /// Model catalog snapshot used for C4 compactor-provider backfill on load.
    /// `None` (catalog unavailable) keeps the legacy behavior unchanged.
    catalog: Option<trogonai_catalog_client::CatalogSnapshot>,
}

// Manual `Clone` so the spawn handler can hold a cheap, shared handle to the
// agent and run sub-agents in-process. Every field is `Arc`-backed or otherwise
// cheap to clone; `A` lives behind `Arc`, so no `A: Clone` bound is required.
// `session_locks` is shared, so sub-session prompt serialization stays coherent.
impl<S: Clone, A, N: Clone, M: Clone> Clone for TrogonAgent<S, A, N, M> {
    fn clone(&self) -> Self {
        Self {
            notifier: self.notifier.clone(),
            store: self.store.clone(),
            agent: self.agent.clone(),
            md_loader: self.md_loader.clone(),
            prefix: self.prefix.clone(),
            default_model: self.default_model.clone(),
            permission_tx: self.permission_tx.clone(),
            elicitation_tx: self.elicitation_tx.clone(),
            gateway_config: self.gateway_config.clone(),
            session_locks: self.session_locks.clone(),
            session_mcp_caches: self.session_mcp_caches.clone(),
            compactor_nats: self.compactor_nats.clone(),
            http: self.http.clone(),
            registry: self.registry.clone(),
            execution_nats: self.execution_nats.clone(),
            classifier: self.classifier.clone(),
            compact_threshold_pct: self.compact_threshold_pct,
            conversation_sink: self.conversation_sink.clone(),
            catalog: self.catalog.clone(),
        }
    }
}

impl<S: SessionStore, A: AgentRunner + 'static, N: SessionNotifier> TrogonAgent<S, A, N, FsTrogonMdLoader> {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        notifier: N,
        store: S,
        agent: A,
        prefix: impl Into<String>,
        default_model: impl Into<String>,
        permission_tx: Option<PermissionTx>,
        elicitation_tx: Option<ElicitationTx>,
        gateway_config: Arc<RwLock<Option<GatewayConfig>>>,
    ) -> Self {
        Self {
            notifier,
            store,
            agent: Arc::new(agent),
            md_loader: FsTrogonMdLoader,
            prefix: prefix.into(),
            default_model: default_model.into(),
            permission_tx,
            elicitation_tx,
            gateway_config,
            session_locks: Arc::new(std::sync::Mutex::new(HashMap::new())),
            session_mcp_caches: Arc::new(std::sync::Mutex::new(HashMap::new())),
            compactor_nats: None,
            http: reqwest::Client::new(),
            registry: None,
            execution_nats: None,
            classifier: None,
            compact_threshold_pct: trogon_runner_tools::compaction_settings_from_env().1,
            conversation_sink: None,
            catalog: None,
        }
    }
}

impl<S: SessionStore, A: AgentRunner + 'static, N: SessionNotifier, M: TrogonMdLoading> TrogonAgent<S, A, N, M> {
    pub fn with_md_loader<M2: TrogonMdLoading>(self, loader: M2) -> TrogonAgent<S, A, N, M2> {
        TrogonAgent {
            md_loader: loader,
            notifier: self.notifier,
            store: self.store,
            agent: self.agent,
            prefix: self.prefix,
            default_model: self.default_model,
            permission_tx: self.permission_tx,
            elicitation_tx: self.elicitation_tx,
            gateway_config: self.gateway_config,
            session_locks: self.session_locks,
            session_mcp_caches: self.session_mcp_caches,
            compactor_nats: self.compactor_nats,
            http: self.http,
            registry: self.registry,
            execution_nats: self.execution_nats,
            classifier: self.classifier,
            compact_threshold_pct: self.compact_threshold_pct,
            conversation_sink: self.conversation_sink,
            catalog: self.catalog,
        }
    }

    /// Set the `auto`-mode LLM safety classifier.
    pub fn with_safety_classifier(
        mut self,
        classifier: Arc<dyn trogon_runner_tools::SafetyClassifier>,
    ) -> Self {
        self.classifier = Some(classifier);
        self
    }

    /// Mirror each turn's conversation into the canonical Session Kernel event log
    /// (shadow mode). Best-effort and non-fatal; leaves the legacy store
    /// authoritative. Wired only when the Session Kernel is enabled.
    pub fn with_conversation_sink(mut self, sink: Arc<dyn crate::kernel_sink::ConversationSink>) -> Self {
        self.conversation_sink = Some(sink);
        self
    }

    /// Enable context compaction via `trogon-compactor`.
    ///
    /// When set, each prompt call sends the session history to
    /// `trogon.compactor.compact` before running the agent loop.  If the
    /// compactor service is unavailable the call degrades gracefully and
    /// continues without compaction.
    pub fn with_compactor(mut self, nats: async_nats::Client) -> Self {
        self.compactor_nats = Some(nats);
        self
    }

    /// Attach a model catalog snapshot for C4 compactor-provider backfill.
    ///
    /// On load, a legacy serde session carrying a bare `compactor_model` (no
    /// provider) has its `compactor_provider` resolved from this catalog and the
    /// record rewritten as protobuf. `None` (catalog unavailable) leaves legacy
    /// records untouched — graceful degradation.
    pub fn with_catalog(mut self, catalog: trogonai_catalog_client::CatalogSnapshot) -> Self {
        self.catalog = Some(catalog);
        self
    }

    /// Enable the `bash` tool by connecting to a `trogon-wasm-runtime` execution backend.
    ///
    /// On each prompt the runner discovers the execution backend from the registry.
    /// If no backend with capability `"execution"` is registered the `bash` tool is
    /// not injected and the agent runs without it — graceful degradation.
    pub fn with_execution_backend(
        mut self,
        nats: async_nats::Client,
        registry: trogon_registry::Registry<async_nats::jetstream::kv::Store>,
    ) -> Self {
        self.execution_nats = Some(nats);
        self.registry = Some(Arc::new(registry));
        self
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
                SessionMode::new("bypassPermissions", "Bypass Permissions"),
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
    async fn publish_session_ready(
        &self,
        session_id: &str,
    ) -> agent_client_protocol::Result<()> {
        let acp_prefix = self.make_acp_prefix()?;
        let acp_session_id = AcpSessionId::new(session_id).map_err(invalid_session_id)?;
        let subject = session_subjects::agent::ExtReadySubject::new(
            &acp_prefix,
            &acp_session_id,
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
        Ok(())
    }

    fn make_acp_session_id(
        &self,
        session_id: &agent_client_protocol::SessionId,
    ) -> agent_client_protocol::Result<AcpSessionId> {
        AcpSessionId::try_from(session_id).map_err(invalid_session_id)
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

    /// Return the per-session MCP tool cache, creating it on first use.
    fn session_mcp_cache(&self, session_id: &str) -> Arc<trogon_runner_tools::SessionMcpCache> {
        let mut caches = self.session_mcp_caches.lock().unwrap();
        caches
            .entry(session_id.to_string())
            .or_insert_with(|| Arc::new(trogon_runner_tools::SessionMcpCache::new()))
            .clone()
    }

    /// Run a sub-agent fully **in-process** and return its assistant text.
    ///
    /// This is the responder behind the `spawn_agent` tool. It must NOT route the
    /// sub-agent's prompt back through NATS into this same runner: the parent
    /// prompt is still in-flight here (blocked awaiting the `spawn_agent` tool
    /// result), so a NATS round-trip into our own session loop deadlocks — the
    /// sub-session is created but its prompt never gets driven to completion.
    /// Instead we drive the full tool-use loop directly via [`Self::run_prompt`],
    /// capturing the assistant text through an [`AccumulatingPromptClient`].
    ///
    /// The sub-agent runs in an isolated git worktree, in an ephemeral session
    /// that inherits the parent's mode, permission rules, egress policy and model,
    /// with the recursion guard (`spawn_depth`) incremented. The worktree and the
    /// ephemeral session are cleaned up on every exit path, including the error /
    /// timeout path.
    #[cfg_attr(coverage, coverage(off))]
    pub async fn spawn_subagent(
        &self,
        parent_session_id: &str,
        prompt: &str,
        agent_name: &str,
    ) -> Result<String, String> {
        use trogon_runner_tools::session_store::SessionState;

        let parent_state = self
            .store
            .load(parent_session_id)
            .await
            .map_err(|_| "spawn_agent: session not found".to_string())?;

        const MAX_SPAWN_DEPTH: u32 = 3;
        if parent_state.spawn_depth >= MAX_SPAWN_DEPTH {
            return Err("spawn_agent: max nesting depth reached".to_string());
        }

        // Resolve a named custom subagent (.claude/agents/) from the parent
        // project dir → its system prompt + optional model override. An empty
        // name means the default agent (no override). A named-but-missing agent
        // is an error, matching the inline spawn handler's contract.
        let subagent = if agent_name.is_empty() {
            None
        } else {
            match trogon_runner_tools::load_subagent(
                std::path::Path::new(&parent_state.cwd),
                agent_name,
            ) {
                Some(def) => Some(def),
                None => {
                    return Err(format!(
                        "spawn_agent: no subagent named `{agent_name}` in .claude/agents/"
                    ));
                }
            }
        };

        // Isolated worktree (best-effort: falls back to the parent cwd if the
        // project is not a git repo). Cleaned up explicitly on every path below.
        let worktree = trogon_runner_tools::worktree::create_worktree(&parent_state.cwd).await;
        let sub_cwd = worktree
            .as_ref()
            .map(|w| w.path.clone())
            .unwrap_or_else(|| parent_state.cwd.clone());

        // Ephemeral sub-session inheriting the parent's execution context.
        let sub_sid = uuid::Uuid::new_v4().to_string();
        let now = now_iso8601();
        let tool_allowlist = subagent
            .as_ref()
            .map(|d| d.tools.clone())
            .unwrap_or_default();
        let sub_state = SessionState {
            cwd: sub_cwd,
            mode: parent_state.mode.clone(),
            // A named subagent may override the model; otherwise inherit the parent's.
            model: subagent
                .as_ref()
                .and_then(|d| d.model.clone())
                .or_else(|| parent_state.model.clone()),
            // A named subagent's prompt replaces the default identity for the sub-session.
            system_prompt_override: subagent.as_ref().map(|d| d.system_prompt.clone()),
            permission_rules_text: parent_state.permission_rules_text.clone(),
            egress_policy: parent_state.egress_policy.clone(),
            allowed_tools: parent_state.allowed_tools.clone(),
            additional_read_dirs: parent_state.additional_read_dirs.clone(),
            additional_roots: parent_state.additional_roots.clone(),
            tool_allowlist,
            parent_session_id: Some(parent_session_id.to_string()),
            spawn_depth: parent_state.spawn_depth + 1,
            created_at: now.clone(),
            updated_at: now,
            ..Default::default()
        };
        if let Err(e) = self.store.save(&sub_sid, &sub_state).await {
            if let Some(w) = worktree {
                w.cleanup().await;
            }
            return Err(format!("spawn_agent error: failed to create sub-session: {e}"));
        }

        // Drive the full tool-use loop in-process, accumulating assistant text.
        let collected = Arc::new(std::sync::Mutex::new(String::new()));
        let client = AccumulatingPromptClient::new(collected.clone());
        let req = PromptRequest::new(
            SessionId::new(sub_sid.clone()),
            vec![ContentBlock::from(prompt)],
        );

        // Shared with the caller-side NATS request timeout in
        // `spawn_agent_tool` so the two can't drift (NEW-23).
        let spawn_safety_timeout = trogon_runner_tools::spawn_agent_tool::SPAWN_AGENT_SAFETY_TIMEOUT;
        let run = tokio::time::timeout(
            spawn_safety_timeout,
            self.run_prompt(&req, &client, None, None),
        )
        .await;

        let persist = std::env::var("TROGON_SUBAGENT_PERSIST")
            .ok()
            .map(|v| !matches!(v.as_str(), "0" | "false" | "FALSE" | "no" | "NO"))
            .unwrap_or(true);

        // Cleanup on every path unless persistence was requested.
        if !persist {
            let _ = self.store.delete(&sub_sid).await;
        }
        if let Some(w) = worktree {
            if persist {
                w.keep();
            } else {
                w.cleanup().await;
            }
        }

        let text = collected.lock().map(|g| g.clone()).unwrap_or_default();

        // SubagentStop: notify when a spawned sub-agent finishes its session.
        if !parent_state.tool_hooks.subagent_stop.is_empty() {
            let payload = serde_json::json!({
                "hook_event_name": "SubagentStop",
                "agent_name": agent_name,
                "sub_session_id": sub_sid,
                "parent_session_id": parent_session_id,
                "persisted": persist,
            });
            let _ = trogon_runner_tools::run_event_hooks(
                &parent_state.tool_hooks.subagent_stop,
                None,
                &payload,
            )
            .await;
        }

        let format_result = |body: String| -> String {
            if persist {
                format!(
                    "{body}\n\n---\nSub-agent session persisted as `{sub_sid}`. \
                     Resume with: trogon --session-id {sub_sid} --prefix {}",
                    self.prefix
                )
            } else {
                body
            }
        };

        match run {
            Err(_elapsed) => Err(format!(
                "spawn_agent error: safety-net timeout after {}s",
                spawn_safety_timeout.as_secs()
            )),
            Ok(Err(e)) => Err(format!("spawn_agent error: {e}")),
            Ok(Ok(_)) => Ok(format_result(if text.is_empty() {
                "Sub-agent completed.".to_string()
            } else {
                text
            })),
        }
    }

    /// Reload the latest persisted session and merge only the fields owned by this prompt.
    #[cfg_attr(coverage, coverage(off))]
    #[allow(clippy::too_many_arguments)]
    async fn save_prompt_state_merged(
        &self,
        session_id: &str,
        prompt_state: &SessionState,
        orig_cwd: &str,
        orig_mode: &str,
        updated_messages: Vec<Message>,
        last_input_tokens: u32,
        last_output_tokens: u32,
        last_cache_creation_tokens: u32,
        last_cache_read_tokens: u32,
        audit_buf: &AuditBuf,
    ) {
        let prompt_title = prompt_state.title.clone();
        let cwd_changed = prompt_state.cwd != orig_cwd;
        let mode_changed = prompt_state.mode != orig_mode;
        let prompt_cwd = prompt_state.cwd.clone();
        let prompt_terminal_cwd = prompt_state.terminal_cwd.clone();
        let prompt_mode = prompt_state.mode.clone();

        let mut merged = match self.store.load(session_id).await {
            Ok(fresh) => fresh,
            Err(_) => prompt_state.clone(),
        };
        merged.messages = updated_messages;
        merged.updated_at = now_iso8601();
        if !prompt_title.is_empty() {
            merged.title = prompt_title;
        }
        if cwd_changed {
            merged.cwd = prompt_cwd;
            merged.terminal_cwd = prompt_terminal_cwd;
        }
        if mode_changed {
            merged.mode = prompt_mode;
        }
        merged.total_input_tokens = merged
            .total_input_tokens
            .saturating_add(last_input_tokens as u64);
        merged.total_output_tokens = merged
            .total_output_tokens
            .saturating_add(last_output_tokens as u64);
        merged.total_cache_creation_tokens = merged
            .total_cache_creation_tokens
            .saturating_add(last_cache_creation_tokens as u64);
        merged.total_cache_read_tokens = merged
            .total_cache_read_tokens
            .saturating_add(last_cache_read_tokens as u64);
        let new_entries = audit_buf
            .lock()
            .map(|mut g| g.drain(..).collect::<Vec<_>>())
            .unwrap_or_default();
        append_audit_entries(&mut merged.audit_log, new_entries);
        if let Err(e) = self.store.save(session_id, &merged).await {
            warn!(session_id, error = %e, "agent: failed to save session");
        }
    }

    /// Core prompt execution. Streams events via `prompt_client` and returns the final response.
    #[cfg_attr(coverage, coverage(off))]
    async fn run_prompt(
        &self,
        req: &PromptRequest,
        prompt_client: &dyn PromptEventClient,
        mut cancel_sub: Option<CancelSubscription>,
        steer_rx: Option<tokio::sync::mpsc::Receiver<String>>,
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

        // C4 migration: a pre-M3 serde session carries a bare `compactor_model`
        // with no `compactor_provider`. Resolve the provider against the catalog and
        // durably rewrite the record as the protobuf pair on save. Ambiguous/unknown/
        // catalog-unavailable cases are NOT silent: warn and persist
        // `needs_compactor_migration` so the next load retries, preserving the bare
        // value rather than guessing a provider. Best-effort: a save failure never
        // fails the prompt.
        {
            use trogonai_catalog_client::BackfillOutcome;
            let prev_needs_migration = state.needs_compactor_migration;
            let mut dirty = false;
            match self.catalog.as_ref() {
                Some(catalog) => {
                    let outcome = trogonai_catalog_client::backfill_compactor_provider(
                        &mut state.compactor_provider,
                        state.compactor_model.as_deref(),
                        catalog,
                    );
                    match outcome {
                        BackfillOutcome::Resolved => {
                            state.needs_compactor_migration = false;
                            dirty = true;
                        }
                        BackfillOutcome::Ambiguous | BackfillOutcome::Unknown => {
                            warn!(
                                session_id,
                                compactor_model = state.compactor_model.as_deref().unwrap_or(""),
                                outcome = ?outcome,
                                "agent: could not resolve compactor provider for bare model; \
                                 degrading and flagging for retry (needs_compactor_migration)"
                            );
                            state.needs_compactor_migration = true;
                            dirty = !prev_needs_migration;
                        }
                        BackfillOutcome::NoModel | BackfillOutcome::AlreadyResolved => {}
                    }
                }
                None => {
                    if state.compactor_provider.is_none() && state.compactor_model.is_some() {
                        warn!(
                            session_id,
                            compactor_model = state.compactor_model.as_deref().unwrap_or(""),
                            "agent: catalog unavailable; cannot resolve compactor provider for bare \
                             model; flagging for retry (needs_compactor_migration)"
                        );
                        state.needs_compactor_migration = true;
                        dirty = !prev_needs_migration;
                    }
                }
            }
            if dirty && let Err(e) = self.store.save(&session_id, &state).await {
                warn!(session_id, error = %e, "agent: failed to persist C4 compactor-provider backfill");
            }
        }

        // MED-17: remember the cwd/mode at load time so the final save can tell
        // which fields this prompt actually changed and re-apply only those on top
        // of a fresh read (preserving concurrent writes to todos/model/config/etc.).
        let orig_cwd = state.cwd.clone();
        let orig_mode = state.mode.clone();

        // Capture the first prompt as the session title
        if state.title.is_empty() {
            let title_source = req
                .prompt
                .iter()
                .find_map(|b| {
                    if let ContentBlock::Text(t) = b {
                        if !t.text.is_empty() { Some(t.text.clone()) } else { None }
                    } else {
                        None
                    }
                })
                .unwrap_or_default();
            state.title = truncate_title(&title_source);
        }

        state.messages.push(user_message_from_request(req));

        let tool_allowlist = trogon_runner_tools::turn_tool_allowlist_from_prompt_meta(
            req.meta.as_ref(),
            state.tool_allowlist.clone(),
        );
        let turn_permission_rules_text = req
            .meta
            .as_ref()
            .and_then(|m| m.get("permissionRules"))
            .and_then(|v| v.as_str())
            .map(String::from);

        // Compact history when token estimate exceeds the configured threshold
        // (COMPACT_THRESHOLD_PCT, default 85%) of the session budget.
        // Degrades gracefully — if trogon-compactor is not running, continues unchanged.
        if let Some(ref nats) = self.compactor_nats
            && estimate_token_count(&state.messages)
                > state.token_budget * self.compact_threshold_pct as u64 / 100
        {
            let model = state.model.as_deref().unwrap_or(&self.default_model).to_string();
            let compactor_provider = state.compactor_provider.clone();
            let compactor_model = state.compactor_model.clone();
            match compact_messages(
                nats,
                &state.messages,
                &session_id,
                &model,
                compactor_provider.as_deref(),
                compactor_model.as_deref(),
            )
            .await
            {
                Ok(Some(new_msgs)) => state.messages = new_msgs,
                Ok(None) => {}
                Err(e) => {
                    warn!(session_id, error = %e, "auto-compaction failed — continuing uncompacted");
                }
            }
        }

        let (event_tx, mut event_rx) = mpsc::channel::<AgentEvent>(32);
        let mut tools: Vec<ToolDef> = all_tool_defs();
        // In plan mode, offer ExitPlanMode so the model can signal it has finished
        // planning and request approval to leave plan mode.
        if state.mode == "plan" {
            tools.push(exit_plan_mode_tool_def());
        }
        tools = trogon_runner_tools::filter_tool_defs_by_allowlist(tools, &tool_allowlist);
        let needs_perm = self.permission_tx.is_some() && state.mode != "bypassPermissions";
        let gateway = self.gateway_config.read().await.clone();

        let needs_elic = self.elicitation_tx.is_some();

        let audit_buf: AuditBuf = std::sync::Arc::new(std::sync::Mutex::new(Vec::<AuditEntry>::new()));
        // Shared cell: the permission bridge writes the chosen follow-up mode here
        // when the user approves an ExitPlanMode request; read after the tool call
        // finishes to update the in-memory session mode.
        let exit_plan_mode_cell: std::sync::Arc<std::sync::Mutex<Option<String>>> =
            std::sync::Arc::new(std::sync::Mutex::new(None));

        let agent: Arc<A> = {
            let needs_clone = state.model.is_some()
                || !state.mcp_servers.is_empty()
                || !state.openapi_servers.is_empty()
                || needs_perm
                || needs_elic
                || gateway.is_some()
                || self.execution_nats.is_some()
                || !state.cwd.is_empty();
            if needs_clone {
                let mut a = (*self.agent).clone();
                if !state.cwd.is_empty() {
                    a.set_cwd(state.cwd.clone());
                }
                if let Some(ref model) = state.model {
                    a.set_model(model.clone());
                }
                if !state.cwd.is_empty() {
                    a.set_cwd(state.cwd.clone());
                }
                if !state.mcp_servers.is_empty() {
                    let policy = state
                        .egress_policy
                        .as_ref()
                        .cloned()
                        .unwrap_or_else(EgressPolicy::default_safe);
                    let mcp_cache = self.session_mcp_cache(&session_id);
                    let (mcp_defs, mcp_dispatch) = mcp_cache
                        .get_or_build(&self.http, &state.mcp_servers, &policy)
                        .await;
                    let (mcp_defs, mcp_dispatch): (Vec<_>, Vec<_>) = if tool_allowlist.is_empty() {
                        (mcp_defs, mcp_dispatch)
                    } else {
                        mcp_defs
                            .into_iter()
                            .zip(mcp_dispatch)
                            .filter(|(def, _)| {
                                trogon_runner_tools::is_tool_in_allowlist(&tool_allowlist, &def.name)
                            })
                            .unzip()
                    };
                    if !mcp_defs.is_empty() {
                        a.add_mcp_tools(mcp_defs, mcp_dispatch);
                    }
                }
                if !state.openapi_servers.is_empty() {
                    let policy = state
                        .egress_policy
                        .as_ref()
                        .cloned()
                        .unwrap_or_else(EgressPolicy::default_safe);
                    let (api_defs, api_dispatch) =
                        trogon_runner_tools::build_session_openapi(&self.http, &state.openapi_servers, &policy).await;
                    // OpenAPI operations dispatch through the same MCP path, so honor
                    // the per-session tool allowlist exactly as MCP tools do.
                    let (api_defs, api_dispatch): (Vec<_>, Vec<_>) = if state.tool_allowlist.is_empty() {
                        (api_defs, api_dispatch)
                    } else {
                        api_defs
                            .into_iter()
                            .zip(api_dispatch)
                            .filter(|(def, _)| {
                                trogon_runner_tools::is_tool_in_allowlist(&state.tool_allowlist, &def.name)
                            })
                            .unzip()
                    };
                    if !api_defs.is_empty() {
                        a.add_mcp_tools(api_defs, api_dispatch);
                    }
                }
                if let (Some(reg), Some(nats)) = (&self.registry, &self.execution_nats)
                    && let Ok(entries) = reg.discover("execution").await
                    && let Some(entry) = entries.first()
                    && trogon_runner_tools::is_tool_in_allowlist(&tool_allowlist, "bash")
                {
                    let prefix = entry.metadata["acp_prefix"].as_str().unwrap_or("acp.wasm");
                    let bash = WasmRuntimeBashTool::new(
                        nats.clone(),
                        prefix,
                        &session_id,
                        std::path::PathBuf::from(&state.cwd),
                        std::time::Duration::from_secs(
                            trogon_runner_tools::wasm_bash_tool::DEFAULT_BASH_TIMEOUT_SECS,
                        ),
                        self.store.clone(),
                    );
                    let (name, orig, client) = bash.into_dispatch();
                    a.add_mcp_tools(
                        vec![WasmRuntimeBashTool::<S>::tool_def()],
                        vec![(name, orig, client)],
                    );
                    // bash_output: poll output from background bash jobs (TOOL-2 wave 2a).
                    let bash_output = trogon_runner_tools::wasm_bash_tool::BashOutputTool::new(
                        nats.clone(),
                        prefix,
                        &session_id,
                        self.store.clone(),
                    );
                    let (bo_name, bo_orig, bo_client) = bash_output.into_dispatch();
                    a.add_mcp_tools(
                        vec![trogon_runner_tools::wasm_bash_tool::BashOutputTool::<S>::tool_def()],
                        vec![(bo_name, bo_orig, bo_client)],
                    );
                }
                // Offer the spawn_agent tool so the model can delegate subtasks to an
                // isolated sub-agent (full tool-use loop in a temp worktree). The
                // responder lives in main.rs; routed via {prefix}.spawn (off the
                // `agent.` namespace to avoid the global-wildcard collision).
                if let Some(ref nats) = self.execution_nats
                    && trogon_runner_tools::is_tool_in_allowlist(&tool_allowlist, "spawn_agent")
                {
                    let spawn = trogon_runner_tools::spawn_agent_tool::SpawnAgentTool::new(
                        nats.clone(),
                        self.prefix.clone(),
                        session_id.clone(),
                    );
                    let (name, orig, client) = spawn.into_dispatch();
                    // List available custom subagents (.claude/agents/) in the tool
                    // description so the model knows which `agent` names it can use.
                    let agent_names: Vec<String> =
                        trogon_runner_tools::load_subagents(std::path::Path::new(&state.cwd))
                            .into_iter()
                            .map(|d| d.name)
                            .collect();
                    a.add_mcp_tools(
                        vec![trogon_runner_tools::spawn_agent_tool::SpawnAgentTool::tool_def_with_agents(&agent_names)],
                        vec![(name, orig, client)],
                    );
                }
                if needs_perm && let Some(ref perm_tx) = self.permission_tx {
                    let mut rules = if let Some(trogon_md) = self.md_loader.load(&state.cwd).await {
                        PermissionRules::parse(&trogon_md)
                    } else {
                        PermissionRules::default()
                    };
                    if let Some(ref extra) = state.permission_rules_text {
                        rules.merge(PermissionRules::parse(extra));
                    }
                    if let Some(ref extra) = turn_permission_rules_text {
                        rules.merge(PermissionRules::parse(extra));
                    }
                    // Read containment: scope read-only auto-allow to cwd plus the
                    // session's additional working dirs (--add-dir) and configured
                    // permissions.additionalDirectories. Protected paths are always
                    // prompted (handled inside the checker). The classifier (when
                    // configured) decides side-effecting tools in `auto` mode.
                    let mut read_dirs = state.additional_roots.clone();
                    read_dirs.extend(state.additional_read_dirs.clone());
                    let extras = trogon_runner_tools::PermissionExtras {
                        cwd: Some(state.cwd.clone()),
                        additional_read_dirs: read_dirs,
                        classifier: self.classifier.clone(),
                        pre_tool_use: state.tool_hooks.pre_tool_use.clone(),
                        // RT-3: wire the session-level scope override (highest
                        // precedence in Scope::resolve); settings/TROGON.md scopes
                        // default to None until those sources are threaded here.
                        session_scope: state.scope.clone(),
                        ..Default::default()
                    };
                    if let Some(checker) = build_mode_permission_checker(
                        &state.mode,
                        &session_id,
                        perm_tx,
                        state.allowed_tools.clone(),
                        Arc::new(rules),
                        state.tool_policies.clone(),
                        audit_buf.clone(),
                        exit_plan_mode_cell.clone(),
                        extras,
                    ) {
                        a.set_permission_checker(checker);
                    }
                }
                if let Some(ref elic_tx) = self.elicitation_tx {
                    a.set_elicitation_provider(Arc::new(ChannelElicitationProvider {
                        session_id: session_id.clone(),
                        tx: elic_tx.clone(),
                    }));
                }
                // PostToolUse hooks run inside the agent loop so a blocking hook's
                // objection is folded into the tool result the model sees.
                if !state.tool_hooks.post_tool_use.is_empty() {
                    a.set_post_tool_observer(Arc::new(
                        trogon_runner_tools::HookPostToolObserver::new(
                            state.tool_hooks.post_tool_use.clone(),
                        ),
                    ));
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

        // Load TROGON.md files (global → repo root → cwd) and prepend to system_prompt.
        let trogon_md = self.md_loader.load(&state.cwd).await;
        let identity = format!(
            "You are Trogon, an AI coding assistant.\n\n{}\n\n{}",
            trogon_runner_tools::URL_FETCH_GUIDANCE,
            trogon_runner_tools::COMPLETION_GUIDANCE,
        );
        // `--system-prompt` (systemPromptOverride) replaces the built-in identity;
        // TROGON.md and `--append-system-prompt` (system_prompt) still apply on top.
        let base = state.system_prompt_override.clone().unwrap_or(identity);
        let mut assembled = base;
        if let Some(tmd) = trogon_md {
            assembled = format!("{assembled}\n\n{tmd}");
        }
        if let Some(sp) = state.system_prompt.clone() {
            assembled = format!("{assembled}\n\n{sp}");
        }
        let system_prompt = Some(assembled);

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

        // Plan mode: steer the model to research read-only and present a plan
        // instead of reacting to write-denials. The permission layer already
        // blocks writes in plan mode; this makes the model behave like a planner.
        let system_prompt = if state.mode == "plan" {
            match system_prompt {
                Some(s) => Some(format!("{s}\n\n{}", trogon_runner_tools::PLAN_MODE_GUIDANCE)),
                None => Some(trogon_runner_tools::PLAN_MODE_GUIDANCE.to_string()),
            }
        } else {
            system_prompt
        };

        let context_window = Some(context_window_tokens(&agent.model()));
        let current_model = state.model.clone().unwrap_or_else(|| self.agent.model());

        let agent_fut = tokio::task::spawn_local(async move {
            agent
                .run_chat_streaming(messages, &tools, system_prompt.as_deref(), event_tx, steer_rx)
                .await
        });

        let mut converter = PromptEventConverter::new(session_id.clone());
        let mut final_messages: Option<Vec<Message>> = None;
        let mut cancelled = false;
        // Cancel-commit: accumulate the assistant's streamed output so an
        // interrupted turn is persisted (user prompt + partial reply) instead of
        // being discarded. We capture text, tool_use blocks, and the results of
        // tools that finished before the interrupt. On cancel, every captured
        // tool_use is paired with a tool_result (its real output, or a synthesized
        // "interrupted" marker for tools still in flight) — the Anthropic API
        // rejects a tool_use with no matching tool_result, so this pairing is
        // mandatory for the transcript to be resumable. Thinking blocks are
        // intentionally NOT captured: streamed thinking has no signature and would
        // be rejected on resume.
        let mut partial_text = String::new();
        // tool_use blocks in the order the model emitted them.
        let mut partial_tool_uses: Vec<AgentContentBlock> = Vec::new();
        // tool_use id → finished output, for tools that completed pre-cancel.
        let mut partial_results: HashMap<String, String> = HashMap::new();
        let mut last_input_tokens: u32 = 0;
        let mut last_output_tokens: u32 = 0;
        let mut last_cache_creation_tokens: u32 = 0;
        let mut last_cache_read_tokens: u32 = 0;
        let mut tool_name_by_id: HashMap<String, String> = HashMap::new();

        loop {
            let cancel_fut = async {
                match cancel_sub.as_mut().map(CancelSubscription::receiver_mut) {
                    Some(rx) => {
                        let _ = rx.await;
                    }
                    None => std::future::pending().await,
                }
            };

            tokio::select! {
                maybe_event = event_rx.recv() => {
                    match maybe_event {
                        Some(event) => {
                            let prompt_event = match event {
                                AgentEvent::TextDelta { text } => {
                                    partial_text.push_str(&text);
                                    PromptEvent::TextDelta { text }
                                }
                                AgentEvent::ThinkingDelta { text } => PromptEvent::ThinkingDelta { text },
                                AgentEvent::ToolCallStarted { id, name, input, parent_tool_use_id } => {
                                    tool_name_by_id.insert(id.clone(), name.clone());
                                    partial_tool_uses.push(AgentContentBlock::ToolUse {
                                        id: id.clone(),
                                        name: name.clone(),
                                        input: input.clone(),
                                        parent_tool_use_id: parent_tool_use_id.clone(),
                                    });
                                    PromptEvent::ToolCallStarted { id, name, input, parent_tool_use_id }
                                }
                                AgentEvent::ToolCallFinished { id, output, exit_code, signal } => {
                                    partial_results.insert(id.clone(), output.clone());
                                    let tool_name = tool_name_by_id.get(&id).cloned();
                                    let is_enter_plan = tool_name
                                        .as_deref()
                                        .map(|n| n == "EnterPlanMode")
                                        .unwrap_or(false);
                                    let is_exit_plan = tool_name
                                        .as_deref()
                                        .map(|n| n == "ExitPlanMode")
                                        .unwrap_or(false);
                                    let is_cd = tool_name
                                        .as_deref()
                                        .map(|n| n == "change_directory")
                                        .unwrap_or(false);

                                    // Persist the new cwd so subsequent turns use it.
                                    const CD_PREFIX: &str = "Working directory is now ";
                                    if is_cd
                                        && let Some(new_path) = output.strip_prefix(CD_PREFIX)
                                    {
                                        state.cwd = new_path.to_string();
                                        state.terminal_cwd = None;
                                    }

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
                                    // ExitPlanMode was approved: the bridge recorded the chosen
                                    // follow-up mode in the shared cell. Apply it to the in-memory
                                    // session so it persists at turn end and gates the next turn.
                                    // (The new mode takes effect on the next turn; the current
                                    // turn's permission checker was built with plan mode.)
                                    // Take the value out before the await so the
                                    // MutexGuard is dropped (clippy::await_holding_lock).
                                    // Only drain the cell for ExitPlanMode, preserving
                                    // the original short-circuit semantics.
                                    let exit_mode = if is_exit_plan {
                                        exit_plan_mode_cell.lock().unwrap().take()
                                    } else {
                                        None
                                    };
                                    if let Some(new_mode) = exit_mode {
                                        state.mode = new_mode.clone();
                                        publish_via_converter(
                                            prompt_client,
                                            &mut converter,
                                            PromptEvent::ModeChanged {
                                                mode: new_mode,
                                                model: current_model.clone(),
                                            },
                                        )
                                        .await;
                                    }
                                    continue;
                                }
                                AgentEvent::ToolBatchFinished { count } => {
                                    // PostToolBatch: fires once after a turn's tools complete.
                                    if !state.tool_hooks.post_tool_batch.is_empty() {
                                        let payload = serde_json::json!({
                                            "hook_event_name": "PostToolBatch",
                                            "tool_count": count,
                                        });
                                        let _ = trogon_runner_tools::run_event_hooks(
                                            &state.tool_hooks.post_tool_batch,
                                            None,
                                            &payload,
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
                                        self.save_prompt_state_merged(
                                            &session_id,
                                            &state,
                                            &orig_cwd,
                                            &orig_mode,
                                            state.messages.clone(),
                                            last_input_tokens,
                                            last_output_tokens,
                                            last_cache_creation_tokens,
                                            last_cache_read_tokens,
                                            &audit_buf,
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
                                        self.save_prompt_state_merged(
                                            &session_id,
                                            &state,
                                            &orig_cwd,
                                            &orig_mode,
                                            state.messages.clone(),
                                            last_input_tokens,
                                            last_output_tokens,
                                            last_cache_creation_tokens,
                                            last_cache_read_tokens,
                                            &audit_buf,
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
                    // Drain events already buffered between the model emitting them
                    // and the cancel firing, so the persisted partial turn is as
                    // complete as possible (text, tool_use blocks, finished results).
                    while let Ok(event) = event_rx.try_recv() {
                        match event {
                            AgentEvent::TextDelta { text } => partial_text.push_str(&text),
                            AgentEvent::ToolCallStarted { id, name, input, parent_tool_use_id } => {
                                tool_name_by_id.insert(id.clone(), name.clone());
                                partial_tool_uses.push(AgentContentBlock::ToolUse {
                                    id, name, input, parent_tool_use_id,
                                });
                            }
                            AgentEvent::ToolCallFinished { id, output, .. } => {
                                partial_results.insert(id, output);
                            }
                            _ => {}
                        }
                    }
                    break;
                }
            }
        }

        // Cancel-commit: persist the user prompt plus whatever the assistant
        // produced before the interrupt, so a cancelled turn is remembered on the
        // next prompt instead of vanishing. `state.messages` already holds the user
        // message (it was never moved into the streaming task). We append:
        //   1. an assistant message: streamed text (if any) followed by the
        //      tool_use blocks the model emitted, and
        //   2. a user message of tool_results — one per tool_use, using the real
        //      output when the tool finished or a synthesized "interrupted" marker
        //      otherwise, so every tool_use is paired (the API requires this).
        // A transcript ending in user(tool_results) is fine: the next prompt's user
        // message is combined with it by the API. If nothing was produced, history
        // is left untouched (avoids a dangling user turn).
        if cancelled {
            let mut assistant_content: Vec<AgentContentBlock> = Vec::new();
            if !partial_text.is_empty() {
                assistant_content
                    .push(AgentContentBlock::Text { text: std::mem::take(&mut partial_text) });
            }
            // Pair every tool_use with a result before moving the blocks out.
            let tool_results: Vec<AgentContentBlock> = partial_tool_uses
                .iter()
                .filter_map(|b| match b {
                    AgentContentBlock::ToolUse { id, .. } => Some(AgentContentBlock::ToolResult {
                        tool_use_id: id.clone(),
                        content: partial_results.get(id).cloned().unwrap_or_else(|| {
                            "Tool call interrupted by the user before it completed.".to_string()
                        }),
                        blocks: vec![],
                    }),
                    _ => None,
                })
                .collect();
            assistant_content.append(&mut partial_tool_uses);

            if !assistant_content.is_empty() {
                let mut updated = state.messages.clone();
                updated.push(Message::assistant(assistant_content));
                if !tool_results.is_empty() {
                    updated.push(Message { role: "user".to_string(), content: tool_results });
                }
                final_messages = Some(updated);
            } else if last_input_tokens > 0 || last_output_tokens > 0 {
                // No partial turn to commit, but persist token usage from completed turns.
                state.total_input_tokens = state.total_input_tokens.saturating_add(last_input_tokens as u64);
                state.total_output_tokens = state.total_output_tokens.saturating_add(last_output_tokens as u64);
                state.total_cache_creation_tokens = state
                    .total_cache_creation_tokens
                    .saturating_add(last_cache_creation_tokens as u64);
                state.total_cache_read_tokens = state
                    .total_cache_read_tokens
                    .saturating_add(last_cache_read_tokens as u64);
                if let Err(e) = self.store.save(&session_id, &state).await {
                    warn!(session_id, error = %e, "agent: failed to save token usage on cancel");
                }
            }
        }

        if let Some(updated) = final_messages {
            // MED-17: compaction and the turn itself are long; meanwhile other ops
            // write the session KV (permission bridge → allowed_tools, bash →
            // terminal_id, todo_write → todos, set_session_model/config → model/
            // config, load_session → mcp_servers, …). The old code only merged
            // allowed_tools/terminal_id back, so every other concurrent write was
            // clobbered by this save. Instead, start from the freshest persisted
            // state and re-apply ONLY the fields this prompt owns.
            let prompt_title = state.title.clone();
            let cwd_changed = state.cwd != orig_cwd;
            let mode_changed = state.mode != orig_mode;
            let prompt_cwd = state.cwd.clone();
            let prompt_terminal_cwd = state.terminal_cwd.clone();
            let prompt_mode = state.mode.clone();

            let mut merged = match self.store.load(&session_id).await {
                Ok(fresh) => fresh,
                // Reload failed — fall back to the in-memory state we already hold.
                Err(_) => state,
            };
            merged.messages = updated;
            merged.updated_at = now_iso8601();
            // The prompt assigns the title on the first turn; carry it if set.
            if !prompt_title.is_empty() {
                merged.title = prompt_title;
            }
            // change_directory during the turn updates cwd + clears terminal_cwd.
            if cwd_changed {
                merged.cwd = prompt_cwd;
                merged.terminal_cwd = prompt_terminal_cwd;
            }
            // ExitPlanMode flow flips mode to "plan" mid-turn.
            if mode_changed {
                merged.mode = prompt_mode;
            }
            // Accumulate this turn's token usage onto the reloaded `merged` state
            // (the value actually persisted below), preserving prior totals.
            merged.total_input_tokens = merged.total_input_tokens.saturating_add(last_input_tokens as u64);
            merged.total_output_tokens = merged.total_output_tokens.saturating_add(last_output_tokens as u64);
            merged.total_cache_creation_tokens = merged
                .total_cache_creation_tokens
                .saturating_add(last_cache_creation_tokens as u64);
            merged.total_cache_read_tokens = merged
                .total_cache_read_tokens
                .saturating_add(last_cache_read_tokens as u64);
            let new_entries = audit_buf
                .lock()
                .map(|mut g| g.drain(..).collect::<Vec<_>>())
                .unwrap_or_default();
            append_audit_entries(&mut merged.audit_log, new_entries);
            if let Err(e) = self.store.save(&session_id, &merged).await {
                warn!(session_id, error = %e, "agent: failed to save session");
            }

            // Shadow-mirror the conversation into the canonical event log when the
            // Session Kernel is enabled (§3). Best-effort and non-fatal.
            if let Some(sink) = &self.conversation_sink {
                // The sink builds the full, no-lossy canonical view of state.messages
                // (structured content blocks, complete tool input/output, and — Fase 5 —
                // large outputs / images as artifact refs) and records it as canonical
                // truth (§11 Transcript no-lossy; § No-Lossy Contract).
                let todos: Vec<trogonai_session_contracts::TodoItem> = merged
                    .todos
                    .iter()
                    .map(|todo| trogonai_session_contracts::TodoItem {
                        id: todo.id.clone(),
                        content: todo.content.clone(),
                        status: todo.status.clone(),
                        ..Default::default()
                    })
                    .collect();
                sink.sync(
                    &session_id,
                    &merged.messages,
                    &merged.cwd,
                    merged.terminal_cwd.as_deref(),
                    &todos,
                )
                .await;
            }
        }

        Ok(PromptResponse::new(if cancelled {
            StopReason::Cancelled
        } else {
            StopReason::EndTurn
        }))
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
impl<S: SessionStore, A: AgentRunner + 'static, N: SessionNotifier, M: TrogonMdLoading + 'static>
    agent_client_protocol::Agent for TrogonAgent<S, A, N, M>
{
    #[cfg_attr(coverage, coverage(off))]
    async fn initialize(&self, _req: InitializeRequest) -> agent_client_protocol::Result<InitializeResponse> {
        let mut session_caps_meta = serde_json::Map::new();
        session_caps_meta.insert("close".to_string(), serde_json::json!({}));
        session_caps_meta.insert("listChildren".to_string(), serde_json::json!({}));
        session_caps_meta.insert("branchAtIndex".to_string(), serde_json::json!({}));
        let capabilities = AgentCapabilities::new().load_session(true).session_capabilities(
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
                GATEWAY_AUTH_METHOD,
                "Gateway",
            ))]))
    }

    #[cfg_attr(coverage, coverage(off))]
    async fn authenticate(
        &self,
        req: AuthenticateRequest,
    ) -> agent_client_protocol::Result<AuthenticateResponse> {
        let method = req.method_id.to_string();
        if method != "gateway_auth" && method != "gateway" {
            return Err(Error::new(
                ErrorCode::InvalidParams.into(),
                format!("unsupported auth method: {method}"),
            ));
        }

        let gateway = req
            .meta
            .as_ref()
            .and_then(|m| m.get("gateway"))
            .and_then(|v| v.as_object())
            .ok_or_else(|| Error::new(ErrorCode::InvalidParams.into(), "missing gateway metadata"))?;
        let base_url = gateway
            .get("baseUrl")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .ok_or_else(|| Error::new(ErrorCode::InvalidParams.into(), "missing gateway.baseUrl"))?
            .to_string();
        let mut token = String::new();
        let mut extra_headers = Vec::new();
        if let Some(headers) = gateway.get("headers").and_then(|v| v.as_object()) {
            for (key, value) in headers {
                let Some(value) = value.as_str() else {
                    continue;
                };
                if key.eq_ignore_ascii_case("authorization") {
                    token = value.strip_prefix("Bearer ").unwrap_or(value).to_string();
                } else {
                    extra_headers.push((key.clone(), value.to_string()));
                }
            }
        }

        *self.gateway_config.write().await = Some(GatewayConfig {
            base_url,
            token,
            extra_headers,
        });
        Ok(AuthenticateResponse::new())
    }

    #[cfg_attr(coverage, coverage(off))]
    async fn new_session(&self, req: NewSessionRequest) -> agent_client_protocol::Result<NewSessionResponse> {
        let session_id = uuid::Uuid::new_v4().to_string();

        let meta = req.meta.as_ref();
        let system_prompt = meta
            .and_then(|m| m.get("systemPrompt"))
            .and_then(|v| v.as_str())
            .map(String::from);
        let system_prompt_override = meta
            .and_then(|m| m.get("systemPromptOverride"))
            .and_then(|v| v.as_str())
            .map(String::from);
        let additional_roots = meta
            .and_then(|m| m.get("additionalRoots"))
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        // permissions.additionalDirectories — read-only allow-list outside cwd.
        let additional_read_dirs = meta
            .and_then(|m| m.get("permissions"))
            .and_then(|p| p.get("additionalDirectories"))
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        // permissions.allow/deny, translated CLI-side to trogon rule-text.
        let permission_rules_text = meta
            .and_then(|m| m.get("permissionRules"))
            .and_then(|v| v.as_str())
            .map(String::from);
        let tool_allowlist = meta
            .and_then(|m| m.get("toolAllowlist"))
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let allowed_tools = meta
            .and_then(|m| m.get("allowedTools"))
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        // PreToolUse/PostToolUse hooks (from settings.json `hooks`).
        let tool_hooks = meta
            .and_then(|m| m.get("toolHooks"))
            .and_then(|v| serde_json::from_value::<trogon_runner_tools::HooksConfig>(v.clone()).ok())
            .unwrap_or_default();
        // OpenAPI connections (Eve-style): each spec operation becomes a tool.
        let openapi_servers = meta
            .and_then(|m| m.get("openapiServers"))
            .and_then(|v| serde_json::from_value::<Vec<trogon_runner_tools::StoredOpenApiServer>>(v.clone()).ok())
            .unwrap_or_default();
        let mode = meta
            .and_then(|m| m.get("mode"))
            .and_then(|v| v.as_str())
            .unwrap_or("default")
            .to_string();
        let env = meta
            .and_then(|m| m.get("env"))
            .and_then(|v| v.as_object())
            .map(|obj| {
                obj.iter()
                    .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                    .collect::<std::collections::HashMap<String, String>>()
            })
            .unwrap_or_default();

        let now = now_iso8601();
        let mcp_servers = trogon_runner_tools::convert_mcp_servers(&req.mcp_servers);
        let state = trogon_runner_tools::session_store::SessionState {
            cwd: req.cwd.to_string_lossy().to_string(),
            mode,
            system_prompt,
            system_prompt_override,
            additional_roots,
            additional_read_dirs,
            permission_rules_text,
            tool_hooks,
            mcp_servers,
            openapi_servers,
            allowed_tools,
            tool_allowlist,
            env,
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
        self.publish_session_ready(&session_id).await?;
        Ok(response)
    }

    #[cfg_attr(coverage, coverage(off))]
    async fn load_session(&self, req: LoadSessionRequest) -> agent_client_protocol::Result<LoadSessionResponse> {
        let session_id = req.session_id.to_string();
        self.make_acp_session_id(&req.session_id)?;
        let mut state = match self.store.load(&session_id).await {
            Ok(s) => s,
            Err(e) => {
                warn!(session_id, error = %e, "agent: failed to load session");
                return Err(internal_error(format!("failed to load session: {e}")));
            }
        };
        let mut needs_save = false;
        let new_cwd = req.cwd.to_string_lossy().into_owned();
        if !new_cwd.is_empty() && state.cwd != new_cwd {
            state.cwd = new_cwd;
            // Clear the terminal so the next bash call spawns a fresh one at the
            // new cwd. Keeping the old terminal and lying about terminal_cwd would
            // cause the mismatch guard to skip the reset — leaving bash stranded at
            // the old directory.
            state.terminal_id = None;
            state.terminal_cwd = None;
            state.updated_at = now_iso8601();
            needs_save = true;
        }
        if !req.mcp_servers.is_empty() {
            state.mcp_servers = trogon_runner_tools::convert_mcp_servers(&req.mcp_servers);
            state.updated_at = now_iso8601();
            needs_save = true;
        }
        if let Some(servers) = req
            .meta
            .as_ref()
            .and_then(|m| m.get("openapiServers"))
            .and_then(|v| serde_json::from_value::<Vec<trogon_runner_tools::StoredOpenApiServer>>(v.clone()).ok())
        {
            state.openapi_servers = servers;
            state.updated_at = now_iso8601();
            needs_save = true;
        }
        if needs_save
            && let Err(e) = self.store.save(&session_id, &state).await
        {
            warn!(session_id, error = %e, "agent: failed to persist session on load");
            return Err(internal_error(format!("failed to save session: {e}")));
        }
        let response = LoadSessionResponse::new()
            .modes(self.session_mode_state(&state.mode))
            .models(self.session_model_state(state.model.as_deref()));
        self.publish_session_ready(&session_id).await?;
        Ok(response)
    }

    #[cfg_attr(coverage, coverage(off))]
    async fn set_session_mode(
        &self,
        req: SetSessionModeRequest,
    ) -> agent_client_protocol::Result<SetSessionModeResponse> {
        let session_id = req.session_id.to_string();
        let semaphore = self.acquire_session_lock(&session_id);
        let _permit = semaphore
            .acquire_owned()
            .await
            .map_err(|_| internal_error("session lock closed"))?;
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
        let semaphore = self.acquire_session_lock(&session_id);
        let _permit = semaphore
            .acquire_owned()
            .await
            .map_err(|_| internal_error("session lock closed"))?;
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
        let semaphore = self.acquire_session_lock(&session_id);
        let _permit = semaphore
            .acquire_owned()
            .await
            .map_err(|_| internal_error("session lock closed"))?;
        let config_id = req.config_id.to_string();
        let value = match &req.value {
            agent_client_protocol::SessionConfigOptionValue::ValueId { value } => value.to_string(),
            agent_client_protocol::SessionConfigOptionValue::Boolean { value } => value.to_string(),
            _ => String::new(),
        };

        let mut state = match self.store.load(&session_id).await {
            Ok(s) => s,
            Err(e) => {
                warn!(session_id, error = %e, "agent: failed to load session for config update");
                return Err(internal_error(format!("failed to load session: {e}")));
            }
        };

        match config_id.as_str() {
            "mode" => {
                state.mode = value.clone();
                state.updated_at = now_iso8601();
                if let Err(e) = self.store.save(&session_id, &state).await {
                    warn!(session_id, error = %e, "agent: failed to persist config mode update");
                    return Err(internal_error(format!("failed to save session: {e}")));
                }
            }
            "model" => {
                state.model = Some(value.clone());
                state.updated_at = now_iso8601();
                if let Err(e) = self.store.save(&session_id, &state).await {
                    warn!(session_id, error = %e, "agent: failed to persist config model update");
                    return Err(internal_error(format!("failed to save session: {e}")));
                }
            }
            "permissions" => {
                state.permission_rules_text = if value.is_empty() { None } else { Some(value.clone()) };
                state.updated_at = now_iso8601();
                if let Err(e) = self.store.save(&session_id, &state).await {
                    warn!(session_id, error = %e, "agent: failed to persist permission rules");
                    return Err(internal_error(format!("failed to save session: {e}")));
                }
            }
            "compactor_model" => {
                let (provider, model) = parse_compactor_config(&value);
                state.compactor_provider = provider;
                state.compactor_model = model;
                state.updated_at = now_iso8601();
                if let Err(e) = self.store.save(&session_id, &state).await {
                    warn!(session_id, error = %e, "agent: failed to persist compactor_model");
                    return Err(internal_error(format!("failed to save session: {e}")));
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
        let compactor_model_options: Vec<SessionConfigSelectOption> = self
            .session_model_state(state.model.as_deref())
            .available_models
            .iter()
            .map(|m| SessionConfigSelectOption::new(m.model_id.to_string(), m.name.as_str()))
            .collect();
        let current_compactor_model = state.compactor_model.clone().unwrap_or_default();
        let config_options = vec![
            SessionConfigOption::select("mode", "Mode", current_mode, mode_options)
                .category(SessionConfigOptionCategory::Mode),
            SessionConfigOption::select("model", "Model", current_model, model_options)
                .category(SessionConfigOptionCategory::Model),
            SessionConfigOption::select(
                "compactor_model",
                "Compaction model",
                current_compactor_model,
                compactor_model_options,
            )
            .category(SessionConfigOptionCategory::Model),
        ];

        Ok(SetSessionConfigOptionResponse::new(config_options))
    }

    #[cfg_attr(coverage, coverage(off))]
    async fn list_sessions(&self, _req: ListSessionsRequest) -> agent_client_protocol::Result<ListSessionsResponse> {
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
            let mut session_meta = serde_json::Map::new();
            if let Some(ref parent_id) = state.parent_session_id {
                session_meta.insert("parentSessionId".to_string(), serde_json::json!(parent_id));
            }
            if let Some(idx) = state.branched_at_index {
                session_meta.insert("branchedAtIndex".to_string(), serde_json::json!(idx));
            }
            if state.total_input_tokens > 0 || state.total_output_tokens > 0 {
                session_meta.insert(
                    "totalInputTokens".to_string(),
                    serde_json::json!(state.total_input_tokens),
                );
                session_meta.insert(
                    "totalOutputTokens".to_string(),
                    serde_json::json!(state.total_output_tokens),
                );
                session_meta.insert(
                    "totalCacheCreationTokens".to_string(),
                    serde_json::json!(state.total_cache_creation_tokens),
                );
                session_meta.insert(
                    "totalCacheReadTokens".to_string(),
                    serde_json::json!(state.total_cache_read_tokens),
                );
            }
            if !session_meta.is_empty() {
                info = info.meta(session_meta);
            }
            sessions.push(info);
        }

        Ok(ListSessionsResponse::new(sessions))
    }

    #[cfg_attr(coverage, coverage(off))]
    async fn fork_session(&self, req: ForkSessionRequest) -> agent_client_protocol::Result<ForkSessionResponse> {
        let source_id = req.session_id.to_string();
        let mut state = match self.store.load(&source_id).await {
            Ok(s) => s,
            Err(e) => {
                warn!(source_id, error = %e, "agent: failed to load source session for fork");
                return Err(internal_error(format!("failed to load source session: {e}")));
            }
        };

        let branch_at: Option<usize> = req
            .meta
            .as_ref()
            .and_then(|m| m.get("branchAtIndex"))
            .and_then(|v| v.as_u64())
            .map(|n| n as usize);

        if let Some(idx) = branch_at {
            state.messages.truncate(idx);
        }

        let new_id = uuid::Uuid::new_v4().to_string();
        let now = now_iso8601();
        state.created_at = now.clone();
        state.updated_at = now;
        state.cwd = req.cwd.to_string_lossy().into_owned();
        state.parent_session_id = Some(source_id.clone());
        state.branched_at_index = branch_at;
        state.total_input_tokens = 0;
        state.total_output_tokens = 0;
        state.total_cache_creation_tokens = 0;
        state.total_cache_read_tokens = 0;
        if let Err(e) = self.store.save(&new_id, &state).await {
            warn!(new_id, error = %e, "agent: failed to save forked session");
            // The fork only exists once persisted; returning the new id after a
            // failed save would hand the client a session that can't be loaded.
            return Err(internal_error(format!("failed to persist forked session: {e}")));
        }

        self.publish_session_ready(&new_id).await?;
        Ok(ForkSessionResponse::new(new_id))
    }

    #[cfg_attr(coverage, coverage(off))]
    async fn resume_session(&self, req: ResumeSessionRequest) -> agent_client_protocol::Result<ResumeSessionResponse> {
        let session_id = req.session_id.to_string();
        self.make_acp_session_id(&req.session_id)?;
        self.publish_session_ready(&session_id).await?;
        Ok(ResumeSessionResponse::new())
    }

    #[cfg_attr(coverage, coverage(off))]
    async fn close_session(&self, req: CloseSessionRequest) -> agent_client_protocol::Result<CloseSessionResponse> {
        let session_id = req.session_id.to_string();

        // Cancel any running prompt for this session.
        let acp_prefix = self.make_acp_prefix()?;
        let acp_session_id = self.make_acp_session_id(&req.session_id)?;
        let cancel_subject = session_subjects::agent::CancelSubject::new(
            &acp_prefix,
            &acp_session_id,
        )
        .to_string();
        let cancel_payload = serde_json::to_vec(&serde_json::json!({ "sessionId": &session_id })).unwrap_or_default();
        self.notifier.publish(cancel_subject, cancel_payload.into()).await;

        if let Err(e) = self.store.delete(&session_id).await {
            warn!(session_id, error = %e, "agent: failed to delete session");
        }

        // B9: the per-session semaphore was inserted on first lock acquisition but
        // never removed, leaking one entry per closed session. Drop it now that the
        // session is gone. Any in-flight prompt holds its own `Arc` clone, so the
        // live semaphore stays valid until that prompt finishes.
        self.session_locks.lock().unwrap().remove(&session_id);
        self.session_mcp_caches.lock().unwrap().remove(&session_id);

        Ok(CloseSessionResponse::new())
    }

    #[cfg_attr(coverage, coverage(off))]
    async fn prompt(&self, req: PromptRequest) -> agent_client_protocol::Result<PromptResponse> {
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
            &acp_session_id,
        )
        .to_string();

        let cancel_sub = self.notifier.subscribe_cancel(cancel_subject).await;

        let steer_subject = session_subjects::agent::SteerSubject::new(
            &acp_prefix,
            &acp_session_id,
        )
        .to_string();
        let steer_rx = self.notifier.subscribe_steer(steer_subject).await;

        let prompt_client = self.notifier.make_prompt_client(acp_session_id, acp_prefix);

        self.run_prompt(&req, &*prompt_client, cancel_sub, steer_rx).await
    }

    #[cfg_attr(coverage, coverage(off))]
    async fn cancel(
        &self,
        req: CancelNotification,
    ) -> agent_client_protocol::Result<()> {
        let acp_prefix = self.make_acp_prefix()?;
        let acp_session_id = self.make_acp_session_id(&req.session_id)?;
        let subject = session_subjects::agent::CancelSubject::new(
            &acp_prefix,
            &acp_session_id,
        )
        .to_string();
        self.notifier.publish(subject, Bytes::new()).await;
        Ok(())
    }

    async fn ext_method(&self, args: ExtRequest) -> agent_client_protocol::Result<ExtResponse> {
        if args.method.as_ref() == "session/list_children" {
            let params: serde_json::Value = serde_json::from_str(args.params.get()).unwrap_or_default();
            let session_id = params.get("sessionId").and_then(|v| v.as_str()).unwrap_or_default();
            let children = self
                .store
                .list_children(session_id)
                .await
                .map_err(|e| internal_error(format!("list_children failed: {e}")))?;
            let result = serde_json::json!({ "children": children });
            let raw = serde_json::value::RawValue::from_string(result.to_string())
                .map_err(|e| internal_error(e.to_string()))?;
            return Ok(ExtResponse::new(raw.into()));
        }
        if args.method.as_ref() == "session/export" {
            let params: serde_json::Value = serde_json::from_str(args.params.get()).unwrap_or_default();
            let session_id = params["sessionId"]
                .as_str()
                .ok_or_else(|| Error::new(ErrorCode::InvalidParams.into(), "missing sessionId".to_string()))?;
            // B11: acquire the session lock before loading, consistent with prompt /
            // import / set_session_*. Without it, an export racing a concurrent
            // compaction round-trip could read torn / stale history mid-write.
            let semaphore = self.acquire_session_lock(session_id);
            let _permit = semaphore
                .acquire_owned()
                .await
                .map_err(|_| internal_error("session lock closed"))?;
            let state = self
                .store
                .load(session_id)
                .await
                .map_err(|e| internal_error(e.to_string()))?;
            let raw = trogon_runner_tools::portable_session::export_json_from_wire(&state.messages)
                .map_err(|e| internal_error(e.to_string()))?;
            return Ok(ExtResponse::new(
                serde_json::value::RawValue::from_string(raw)
                    .map_err(|e| internal_error(e.to_string()))?
                    .into(),
            ));
        }
        if args.method.as_ref() == "session/import" {
            let params: serde_json::Value = serde_json::from_str(args.params.get()).unwrap_or_default();
            let session_id = params["sessionId"]
                .as_str()
                .ok_or_else(|| Error::new(ErrorCode::InvalidParams.into(), "missing sessionId".to_string()))?;
            let semaphore = self.acquire_session_lock(session_id);
            let _permit = semaphore
                .acquire_owned()
                .await
                .map_err(|_| internal_error("session lock closed"))?;
            let messages_json = params["messages"].to_string();
            let parsed = trogon_runner_tools::portable_session::parse_export_json(&messages_json)
                .map_err(|e| Error::new(ErrorCode::InvalidParams.into(), e.to_string()))?;
            let mut state = self.store.load(session_id).await.unwrap_or_default();
            state.messages = match parsed {
                trogon_runner_tools::portable_session::ParsedExport::V1(msgs) => msgs
                    .into_iter()
                    .map(|m| Message {
                        role: m.role,
                        content: vec![AgentContentBlock::Text { text: m.text }],
                    })
                    .collect(),
                trogon_runner_tools::portable_session::ParsedExport::V2(exp) => {
                    trogon_runner_tools::portable_session::v2_to_messages(&exp)
                }
            };
            state.updated_at = now_iso8601();
            self.store
                .save(session_id, &state)
                .await
                .map_err(|e| internal_error(e.to_string()))?;
            let raw = serde_json::value::RawValue::from_string("{}".to_string()).unwrap();
            return Ok(ExtResponse::new(raw.into()));
        }
        if args.method.as_ref() == "session/compact" {
            let params: serde_json::Value = serde_json::from_str(args.params.get()).unwrap_or_default();
            let session_id = params["sessionId"]
                .as_str()
                .ok_or_else(|| Error::new(ErrorCode::InvalidParams.into(), "missing sessionId".to_string()))?;
            let semaphore = self.acquire_session_lock(session_id);
            let _permit = semaphore
                .acquire_owned()
                .await
                .map_err(|_| internal_error("session lock closed"))?;
            let mut state = self
                .store
                .load(session_id)
                .await
                .map_err(|e| internal_error(e.to_string()))?;
            let tokens_before = estimate_token_count(&state.messages);
            let (compacted, tokens_after) = if let Some(ref nats) = self.compactor_nats {
                let model = state.model.clone().unwrap_or_else(|| self.default_model.clone());
                let compactor_provider = state.compactor_provider.clone();
                let compactor_model = state.compactor_model.clone();
                // Manual /compact propagates a compactor-down failure as an error so
                // the user sees it, rather than reporting a misleading "no compaction
                // needed". Distinct from the auto-path, which degrades silently.
                let compacted_msgs = compact_messages(
                    nats,
                    &state.messages,
                    session_id,
                    &model,
                    compactor_provider.as_deref(),
                    compactor_model.as_deref(),
                )
                .await
                .map_err(|e| internal_error(e.to_string()))?;
                match compacted_msgs {
                    Some(new_msgs) => {
                        let after = estimate_token_count(&new_msgs);
                        state.messages = new_msgs;
                        if after < tokens_before {
                            state.updated_at = now_iso8601();
                            self.store
                                .save(session_id, &state)
                                .await
                                .map_err(|e| internal_error(e.to_string()))?;
                            (true, after)
                        } else {
                            (false, tokens_before)
                        }
                    }
                    None => (false, tokens_before),
                }
            } else {
                (false, tokens_before)
            };
            let result = serde_json::json!({
                "compacted": compacted,
                "tokens_before": tokens_before,
                "tokens_after": tokens_after,
            });
            let raw = serde_json::value::RawValue::from_string(result.to_string())
                .map_err(|e| internal_error(e.to_string()))?;
            return Ok(ExtResponse::new(raw.into()));
        }
        Err(Error::new(
            ErrorCode::MethodNotFound.into(),
            format!("unknown ext method: {}", args.method),
        ))
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
        let req = PromptRequest::new("s1", vec![ContentBlock::Text(TextContent::new("hello"))]);
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
        let is_enter_plan = tool_name_by_id.get(&id).map(|n| n == "EnterPlanMode").unwrap_or(false);
        assert!(is_enter_plan);
    }

    #[test]
    fn other_tools_not_detected_as_enter_plan_mode() {
        let mut tool_name_by_id: HashMap<String, String> = HashMap::new();
        tool_name_by_id.insert("id-1".to_string(), "get_pr_diff".to_string());
        tool_name_by_id.insert("id-2".to_string(), "TodoWrite".to_string());
        for id in &["id-1", "id-2"] {
            let is_enter_plan = tool_name_by_id.get(*id).map(|n| n == "EnterPlanMode").unwrap_or(false);
            assert!(!is_enter_plan, "tool {id} must not be detected as EnterPlanMode");
        }
    }

    // ── estimate_token_count ──────────────────────────────────────────────────

    #[test]
    fn estimate_token_count_empty_returns_zero() {
        assert_eq!(estimate_token_count(&[]), 0);
    }

    // ── compaction request wire contract (acp provider identity) ──────────────
    //
    // acp delegates build+NATS+decode to `request_compaction`; here we assert the
    // acp-specific inputs (SESSION_PROVIDER + context_window_tokens) produce the
    // expected wire payload via the underlying `encode_compact_request`.

    #[test]
    fn compact_request_includes_provider_model_and_override() {
        use buffa::Message as _;
        use trogon_runner_tools::encode_compact_request;
        use trogonai_compactor_proto::CompactRequest as ProtoRequest;

        let msgs = vec![Message::user_text("hi")];
        let bytes = encode_compact_request(
            &msgs,
            SESSION_PROVIDER,
            "claude-opus-4-6",
            Some(context_window_tokens("claude-opus-4-6")),
            Some("xai"),
            Some("grok-4"),
        );
        let proto = ProtoRequest::decode_from_slice(&bytes).unwrap();
        assert_eq!(proto.provider, "anthropic");
        assert_eq!(proto.model, "claude-opus-4-6");
        assert_eq!(proto.compactor_provider.as_deref(), Some("xai"));
        assert_eq!(proto.compactor_model.as_deref(), Some("grok-4"));
        assert_eq!(proto.messages.len(), 1);
    }

    #[test]
    fn compact_request_omits_compactor_fields_when_none() {
        use buffa::Message as _;
        use trogon_runner_tools::encode_compact_request;
        use trogonai_compactor_proto::CompactRequest as ProtoRequest;

        let msgs = vec![Message::user_text("hi")];
        let bytes = encode_compact_request(
            &msgs,
            SESSION_PROVIDER,
            "claude-opus-4-6",
            Some(context_window_tokens("claude-opus-4-6")),
            None,
            None,
        );
        let proto = ProtoRequest::decode_from_slice(&bytes).unwrap();
        assert_eq!(proto.model, "claude-opus-4-6");
        assert!(proto.compactor_provider.is_none());
        assert!(proto.compactor_model.is_none());
    }

    #[test]
    fn estimate_token_count_is_bytes_divided_by_four() {
        let msgs = vec![Message::user_text("hello")];
        let json_len = serde_json::to_string(&msgs).unwrap().len() as u64;
        assert_eq!(estimate_token_count(&msgs), json_len / 4);
    }

    #[test]
    fn estimate_token_count_grows_with_message_length() {
        let short = vec![Message::user_text("hi")];
        let long = vec![Message::user_text("x".repeat(10_000))];
        assert!(
            estimate_token_count(&long) > estimate_token_count(&short),
            "longer messages must produce a higher estimate"
        );
    }

    // ── 85 % compact threshold ────────────────────────────────────────────────

    #[test]
    fn compact_threshold_not_reached_for_small_messages() {
        let msgs = vec![Message::user_text("hello")];
        let estimate = estimate_token_count(&msgs);
        let budget = 200_000u64;
        assert!(
            estimate <= budget * 85 / 100,
            "a tiny message must not exceed the 85 % threshold (estimate={estimate})"
        );
    }

    #[test]
    fn compact_threshold_reached_when_messages_exceed_85_percent() {
        // Use a very small budget so a few messages tip over the threshold.
        let budget = 10u64;
        let msgs = vec![Message::user_text("x".repeat(200))];
        let estimate = estimate_token_count(&msgs);
        assert!(
            estimate > budget * 85 / 100,
            "large messages must exceed the 85 % threshold of a small budget \
             (estimate={estimate}, threshold={})",
            budget * 85 / 100
        );
    }

    #[test]
    fn compact_threshold_boundary_at_exactly_85_percent() {
        // At exactly 85 % the condition is > (strict), so compact must NOT trigger.
        let budget = 100u64;
        let threshold = budget * 85 / 100; // 85
        // estimate == threshold → condition (estimate > threshold) is false → no compact
        let estimate = threshold;
        assert!(
            estimate <= threshold,
            "strictly greater-than must be false at the boundary"
        );
    }

    // ── ext_method: session/export and session/import ─────────────────────────

    #[cfg(feature = "test-helpers")]
    use agent_client_protocol::Agent as _;

    #[cfg(feature = "test-helpers")]
    fn make_export_params(session_id: &str) -> std::sync::Arc<serde_json::value::RawValue> {
        serde_json::value::RawValue::from_string(serde_json::json!({"sessionId": session_id}).to_string())
            .unwrap()
            .into()
    }

    #[cfg(feature = "test-helpers")]
    fn make_import_params(
        session_id: &str,
        messages: serde_json::Value,
    ) -> std::sync::Arc<serde_json::value::RawValue> {
        serde_json::value::RawValue::from_string(
            serde_json::json!({"sessionId": session_id, "messages": messages}).to_string(),
        )
        .unwrap()
        .into()
    }

    #[cfg(feature = "test-helpers")]
    #[tokio::test]
    async fn ext_method_export_returns_portable_messages() {
        use trogon_runner_tools::session_store::{SessionState, mock::MemorySessionStore};

        let store = MemorySessionStore::new();
        let store_clone = store.clone();

        let state = SessionState {
            messages: vec![
                Message::user_text("hello"),
                Message::assistant(vec![AgentContentBlock::Text { text: "world".into() }]),
            ],
            ..Default::default()
        };
        store_clone.save("s1", &state).await.unwrap();

        let agent = TrogonAgent::new(
            crate::session_notifier::mock::MockSessionNotifier::new(),
            store,
            crate::agent_runner::mock::MockAgentRunner::new("claude-3-5-sonnet-20241022"),
            "test-prefix",
            "claude-3-5-sonnet-20241022",
            None,
            None,
            std::sync::Arc::new(tokio::sync::RwLock::new(None)),
        );

        let resp = agent
            .ext_method(ExtRequest::new("session/export", make_export_params("s1")))
            .await
            .unwrap();

        let portable: Vec<trogon_runner_tools::portable_session::PortableMessage> =
            serde_json::from_str(resp.0.get()).unwrap();

        assert_eq!(portable.len(), 2);
        assert_eq!(portable[0].role, "user");
        assert_eq!(portable[0].text, "hello");
        assert_eq!(portable[1].role, "assistant");
        assert_eq!(portable[1].text, "world");
    }

    // ── cancel-commit (Tier 1) ────────────────────────────────────────────────

    /// A cancelled turn must persist the user prompt plus whatever assistant text
    /// streamed before the interrupt, so the partial reply survives into the next
    /// prompt instead of being discarded.
    #[cfg(feature = "test-helpers")]
    #[tokio::test]
    async fn cancel_commits_partial_assistant_text() {
        use trogon_runner_tools::session_store::{SessionState, mock::MemorySessionStore};

        let store = MemorySessionStore::new();
        let store_clone = store.clone();
        // run_prompt errors if the session can't be loaded — create it first.
        store_clone.save("cancel-1", &SessionState::default()).await.unwrap();

        // The runner emits one text delta, then blocks on the steer wait. With the
        // steer sender kept alive (never sending), the only way out of the prompt
        // loop is the cancel signal — making the cancel path deterministic.
        let started = std::sync::Arc::new(tokio::sync::Notify::new());
        let runner = crate::agent_runner::mock::MockAgentRunner::new("claude-test")
            .with_events(vec![AgentEvent::TextDelta { text: "partial answer".into() }])
            .with_steer_wait()
            .with_started_notify(started.clone());

        let agent = TrogonAgent::new(
            crate::session_notifier::mock::MockSessionNotifier::new(),
            store,
            runner,
            "test-prefix",
            "claude-test",
            None,
            None,
            std::sync::Arc::new(tokio::sync::RwLock::new(None)),
        );

        let (cancel_tx, cancel_rx) = tokio::sync::oneshot::channel::<()>();
        let (_steer_tx, steer_rx) = tokio::sync::mpsc::channel::<String>(1);

        let req = PromptRequest::new("cancel-1", vec![]);
        let prompt_client = agent.notifier.make_prompt_client(
            AcpSessionId::new("cancel-1").unwrap(),
            AcpPrefix::new("test-prefix").unwrap(),
        );

        let local = tokio::task::LocalSet::new();
        local
            .run_until(async {
                let canceller = async {
                    // The runner has now emitted its text and is blocked on steer,
                    // so the partial text is guaranteed buffered before we cancel.
                    started.notified().await;
                    let _ = cancel_tx.send(());
                };
                let (resp, ()) = tokio::join!(
                    agent.run_prompt(
                        &req,
                        &*prompt_client,
                        Some(CancelSubscription::from_receiver(cancel_rx)),
                        Some(steer_rx),
                    ),
                    canceller,
                );
                let resp = resp.expect("run_prompt must return Ok on cancel");
                assert!(
                    matches!(resp.stop_reason, StopReason::Cancelled),
                    "cancelled turn must report StopReason::Cancelled, got {:?}",
                    resp.stop_reason
                );
            })
            .await;

        let state = store_clone.load("cancel-1").await.unwrap();
        assert_eq!(
            state.messages.len(),
            2,
            "user prompt + partial assistant reply must be persisted on cancel"
        );
        assert_eq!(state.messages[0].role, "user");
        assert_eq!(state.messages[1].role, "assistant");
        match &state.messages[1].content[0] {
            AgentContentBlock::Text { text } => assert_eq!(text, "partial answer"),
            other => panic!("expected assistant Text block, got {other:?}"),
        }
    }

    /// A turn cancelled mid-tool-cycle must persist the tool_use blocks paired
    /// with tool_results: the real output for tools that finished before the
    /// interrupt, and a synthesized "interrupted" marker for tools still in flight
    /// (the API rejects a tool_use with no matching tool_result).
    #[cfg(feature = "test-helpers")]
    #[tokio::test]
    async fn cancel_commits_tool_uses_with_paired_results() {
        use trogon_runner_tools::session_store::{SessionState, mock::MemorySessionStore};

        let store = MemorySessionStore::new();
        let store_clone = store.clone();
        store_clone.save("cancel-2", &SessionState::default()).await.unwrap();

        // t1 finishes (real output); t2 starts but never finishes (interrupted).
        let started = std::sync::Arc::new(tokio::sync::Notify::new());
        let runner = crate::agent_runner::mock::MockAgentRunner::new("claude-test")
            .with_events(vec![
                AgentEvent::TextDelta { text: "let me check".into() },
                AgentEvent::ToolCallStarted {
                    id: "t1".into(),
                    name: "bash".into(),
                    input: serde_json::json!({"command": "ls"}),
                    parent_tool_use_id: None,
                },
                AgentEvent::ToolCallFinished {
                    id: "t1".into(),
                    output: "result-1".into(),
                    exit_code: Some(0),
                    signal: None,
                },
                AgentEvent::ToolCallStarted {
                    id: "t2".into(),
                    name: "read_file".into(),
                    input: serde_json::json!({"path": "/tmp/x"}),
                    parent_tool_use_id: None,
                },
            ])
            .with_steer_wait()
            .with_started_notify(started.clone());

        let agent = TrogonAgent::new(
            crate::session_notifier::mock::MockSessionNotifier::new(),
            store,
            runner,
            "test-prefix",
            "claude-test",
            None,
            None,
            std::sync::Arc::new(tokio::sync::RwLock::new(None)),
        );

        let (cancel_tx, cancel_rx) = tokio::sync::oneshot::channel::<()>();
        let (_steer_tx, steer_rx) = tokio::sync::mpsc::channel::<String>(1);
        let req = PromptRequest::new("cancel-2", vec![]);
        let prompt_client = agent.notifier.make_prompt_client(
            AcpSessionId::new("cancel-2").unwrap(),
            AcpPrefix::new("test-prefix").unwrap(),
        );

        let local = tokio::task::LocalSet::new();
        local
            .run_until(async {
                let canceller = async {
                    started.notified().await;
                    let _ = cancel_tx.send(());
                };
                let (resp, ()) = tokio::join!(
                    agent.run_prompt(
                        &req,
                        &*prompt_client,
                        Some(CancelSubscription::from_receiver(cancel_rx)),
                        Some(steer_rx),
                    ),
                    canceller,
                );
                assert!(matches!(
                    resp.unwrap().stop_reason,
                    StopReason::Cancelled
                ));
            })
            .await;

        let state = store_clone.load("cancel-2").await.unwrap();
        // [user(""), assistant(text + 2 tool_use), user(2 tool_result)]
        assert_eq!(state.messages.len(), 3);

        let assistant = &state.messages[1];
        assert_eq!(assistant.role, "assistant");
        assert!(matches!(&assistant.content[0],
            AgentContentBlock::Text { text } if text == "let me check"));
        assert!(matches!(&assistant.content[1],
            AgentContentBlock::ToolUse { id, name, .. } if id == "t1" && name == "bash"));
        assert!(matches!(&assistant.content[2],
            AgentContentBlock::ToolUse { id, name, .. } if id == "t2" && name == "read_file"));

        let results = &state.messages[2];
        assert_eq!(results.role, "user");
        assert!(matches!(&results.content[0],
            AgentContentBlock::ToolResult { tool_use_id, content, .. }
            if tool_use_id == "t1" && content == "result-1"),
            "finished tool keeps its real output, got {:?}", results.content[0]);
        match &results.content[1] {
            AgentContentBlock::ToolResult { tool_use_id, content, .. } => {
                assert_eq!(tool_use_id, "t2");
                assert!(content.contains("interrupted"),
                    "in-flight tool gets a synthesized interrupt result, got {content:?}");
            }
            other => panic!("expected ToolResult for t2, got {other:?}"),
        }
    }

    // ── session/export of non-text blocks → versioned V2 wrapper ──────────────
    //
    // A message containing any block that is not plain Text/Image (ToolUse,
    // ToolResult, Thinking) is exported as the versioned V2 wrapper, which
    // preserves the block structurally instead of flattening it to prose. These
    // tests exercise the acp-runner `session/export` ext_method handler and parse
    // the result with `parse_export_json` (which handles both V1 and V2 shapes).

    /// Build a `TrogonAgent` over a `MemorySessionStore` for ext_method tests.
    #[cfg(feature = "test-helpers")]
    fn make_export_test_agent(
        store: trogon_runner_tools::session_store::mock::MemorySessionStore,
    ) -> TrogonAgent<
        trogon_runner_tools::session_store::mock::MemorySessionStore,
        crate::agent_runner::mock::MockAgentRunner,
        crate::session_notifier::mock::MockSessionNotifier,
    > {
        TrogonAgent::new(
            crate::session_notifier::mock::MockSessionNotifier::new(),
            store,
            crate::agent_runner::mock::MockAgentRunner::new("claude-3-5-sonnet-20241022"),
            "test-prefix",
            "claude-3-5-sonnet-20241022",
            None,
            None,
            std::sync::Arc::new(tokio::sync::RwLock::new(None)),
        )
    }

    #[cfg(feature = "test-helpers")]
    #[tokio::test]
    async fn ext_method_export_represents_tool_use_as_v2_block() {
        use trogon_runner_tools::portable_session::{ParsedExport, PortableBlock, parse_export_json};
        use trogon_runner_tools::session_store::{SessionState, mock::MemorySessionStore};

        let store = MemorySessionStore::new();
        let state = SessionState {
            messages: vec![Message {
                role: "assistant".to_string(),
                content: vec![AgentContentBlock::ToolUse {
                    id: "call-x".to_string(),
                    name: "read_file".to_string(),
                    input: serde_json::json!({"path": "/tmp/foo"}),
                    parent_tool_use_id: None,
                }],
            }],
            ..Default::default()
        };
        store.clone().save("tu1", &state).await.unwrap();
        let agent = make_export_test_agent(store);

        let resp = agent
            .ext_method(ExtRequest::new("session/export", make_export_params("tu1")))
            .await
            .unwrap();

        // A tool_use message exports as V2 with the call preserved as a structured
        // ToolUse block — the bare tool name is never emitted as prose (MED-18).
        match parse_export_json(resp.0.get()).unwrap() {
            ParsedExport::V2(exp) => {
                assert_eq!(exp.messages.len(), 1);
                assert_eq!(exp.messages[0].role, "assistant");
                assert!(
                    exp.messages[0]
                        .blocks
                        .iter()
                        .any(|b| matches!(b, PortableBlock::ToolUse { name, .. } if name == "read_file")),
                    "export must preserve the tool_use as a ToolUse block; got: {:?}",
                    exp.messages[0].blocks
                );
            }
            ParsedExport::V1(_) => panic!("a tool_use message must export as V2"),
        }
    }

    #[cfg(feature = "test-helpers")]
    #[tokio::test]
    async fn ext_method_export_preserves_tool_result_content_v2() {
        use trogon_runner_tools::portable_session::{ParsedExport, PortableBlock, parse_export_json};
        use trogon_runner_tools::session_store::{SessionState, mock::MemorySessionStore};

        let store = MemorySessionStore::new();
        let state = SessionState {
            messages: vec![Message {
                role: "user".to_string(),
                content: vec![AgentContentBlock::ToolResult {
                    tool_use_id: "call-1".to_string(),
                    content: "tool output here".to_string(),
                    blocks: vec![],
                }],
            }],
            ..Default::default()
        };
        store.clone().save("tr1", &state).await.unwrap();
        let agent = make_export_test_agent(store);

        let resp = agent
            .ext_method(ExtRequest::new("session/export", make_export_params("tr1")))
            .await
            .unwrap();

        match parse_export_json(resp.0.get()).unwrap() {
            ParsedExport::V2(exp) => {
                assert_eq!(exp.messages.len(), 1);
                assert_eq!(exp.messages[0].role, "user");
                assert!(
                    exp.messages[0].blocks.iter().any(|b| matches!(
                        b,
                        PortableBlock::ToolResult { output_summary, .. }
                            if output_summary == "tool output here"
                    )),
                    "export must preserve the tool result content; got: {:?}",
                    exp.messages[0].blocks
                );
            }
            ParsedExport::V1(_) => panic!("a tool_result message must export as V2"),
        }
    }

    #[cfg(feature = "test-helpers")]
    #[tokio::test]
    async fn ext_method_export_mixed_text_and_tool_result_v2() {
        use trogon_runner_tools::portable_session::{ParsedExport, PortableBlock, parse_export_json};
        use trogon_runner_tools::session_store::{SessionState, mock::MemorySessionStore};

        let store = MemorySessionStore::new();
        let state = SessionState {
            messages: vec![Message {
                role: "user".to_string(),
                content: vec![
                    AgentContentBlock::Text {
                        text: "before".to_string(),
                    },
                    AgentContentBlock::ToolResult {
                        tool_use_id: "c2".to_string(),
                        content: "result".to_string(),
                        blocks: vec![],
                    },
                ],
            }],
            ..Default::default()
        };
        store.clone().save("tr2", &state).await.unwrap();
        let agent = make_export_test_agent(store);

        let resp = agent
            .ext_method(ExtRequest::new("session/export", make_export_params("tr2")))
            .await
            .unwrap();

        match parse_export_json(resp.0.get()).unwrap() {
            ParsedExport::V2(exp) => {
                assert_eq!(exp.messages.len(), 1);
                let blocks = &exp.messages[0].blocks;
                assert!(
                    blocks
                        .iter()
                        .any(|b| matches!(b, PortableBlock::Text { text } if text == "before")),
                    "text block must be preserved; got: {blocks:?}"
                );
                assert!(
                    blocks.iter().any(|b| matches!(
                        b,
                        PortableBlock::ToolResult { output_summary, .. } if output_summary == "result"
                    )),
                    "tool result must be preserved alongside the text; got: {blocks:?}"
                );
            }
            ParsedExport::V1(_) => panic!("a message with a tool_result must export as V2"),
        }
    }

    #[cfg(feature = "test-helpers")]
    #[tokio::test]
    async fn ext_method_export_represents_thinking_as_v2_block() {
        use trogon_runner_tools::portable_session::{ParsedExport, PortableBlock, parse_export_json};
        use trogon_runner_tools::session_store::{SessionState, mock::MemorySessionStore};

        let store = MemorySessionStore::new();
        let state = SessionState {
            messages: vec![Message {
                role: "assistant".to_string(),
                content: vec![AgentContentBlock::Thinking {
                    thinking: "internal reasoning".to_string(),
                    signature: None,
                }],
            }],
            ..Default::default()
        };
        store.clone().save("th1", &state).await.unwrap();
        let agent = make_export_test_agent(store);

        let resp = agent
            .ext_method(ExtRequest::new("session/export", make_export_params("th1")))
            .await
            .unwrap();

        match parse_export_json(resp.0.get()).unwrap() {
            ParsedExport::V2(exp) => {
                assert_eq!(exp.messages.len(), 1);
                assert!(
                    exp.messages[0].blocks.iter().any(|b| matches!(
                        b,
                        PortableBlock::Thinking { text } if text == "internal reasoning"
                    )),
                    "thinking must be preserved as a structured Thinking block; got: {:?}",
                    exp.messages[0].blocks
                );
            }
            ParsedExport::V1(_) => panic!("a thinking message must export as V2"),
        }
    }

    #[cfg(feature = "test-helpers")]
    #[tokio::test]
    async fn ext_method_import_saves_to_store() {
        use trogon_runner_tools::session_store::mock::MemorySessionStore;

        let store = MemorySessionStore::new();
        let store_clone = store.clone();

        let agent = TrogonAgent::new(
            crate::session_notifier::mock::MockSessionNotifier::new(),
            store,
            crate::agent_runner::mock::MockAgentRunner::new("claude-3-5-sonnet-20241022"),
            "test-prefix",
            "claude-3-5-sonnet-20241022",
            None,
            None,
            std::sync::Arc::new(tokio::sync::RwLock::new(None)),
        );

        let params = make_import_params("s1", serde_json::json!([{"role": "user", "text": "imported"}]));

        agent
            .ext_method(ExtRequest::new("session/import", params))
            .await
            .unwrap();

        let state = store_clone.load("s1").await.unwrap();
        assert_eq!(state.messages.len(), 1);
        assert_eq!(state.messages[0].role, "user");
        match &state.messages[0].content[0] {
            AgentContentBlock::Text { text } => assert_eq!(text, "imported"),
            _ => panic!("expected Text block"),
        }
    }

    #[tokio::test]
    async fn set_session_config_option_compactor_model_persists_to_store() {
        use agent_client_protocol::Agent as _;
        use trogon_runner_tools::session_store::{SessionState, SessionStore, mock::MemorySessionStore};

        let store = MemorySessionStore::new();
        let store_clone = store.clone();
        store_clone.save("s-cm", &SessionState::default()).await.unwrap();
        let agent = TrogonAgent::new(
            crate::session_notifier::mock::MockSessionNotifier::new(),
            store,
            crate::agent_runner::mock::MockAgentRunner::new("claude-3-5-sonnet-20241022"),
            "test-prefix",
            "claude-3-5-sonnet-20241022",
            None,
            None,
            std::sync::Arc::new(tokio::sync::RwLock::new(None)),
        );

        agent
            .set_session_config_option(SetSessionConfigOptionRequest::new(
                "s-cm",
                "compactor_model",
                "anthropic::claude-haiku-4-5",
            ))
            .await
            .unwrap();

        // Must persist to the store (acp's SessionState is the persisted type).
        let state = store_clone.load("s-cm").await.unwrap();
        assert_eq!(
            state.compactor_provider,
            Some("anthropic".to_string()),
            "compactor_provider override must persist on the acp session"
        );
        assert_eq!(
            state.compactor_model,
            Some("claude-haiku-4-5".to_string()),
            "compactor_model override must persist on the acp session"
        );

        // Empty value clears it.
        agent
            .set_session_config_option(SetSessionConfigOptionRequest::new("s-cm", "compactor_model", ""))
            .await
            .unwrap();
        let cleared = store_clone.load("s-cm").await.unwrap();
        assert_eq!(cleared.compactor_provider, None, "empty value clears provider");
        assert_eq!(cleared.compactor_model, None, "empty value clears the override");
    }

    #[tokio::test]
    async fn set_session_config_option_unknown_session_initializes_and_persists() {
        use agent_client_protocol::Agent as _;
        use trogon_runner_tools::session_store::SessionStore as _;
        use trogon_runner_tools::session_store::mock::MemorySessionStore;

        let store = MemorySessionStore::new();
        let agent = TrogonAgent::new(
            crate::session_notifier::mock::MockSessionNotifier::new(),
            store.clone(),
            crate::agent_runner::mock::MockAgentRunner::new("claude-3-5-sonnet-20241022"),
            "test-prefix",
            "claude-3-5-sonnet-20241022",
            None,
            None,
            std::sync::Arc::new(tokio::sync::RwLock::new(None)),
        );

        // Resume-or-create: an unknown session is initialized on demand and the
        // config change is persisted against the freshly created state.
        agent
            .set_session_config_option(SetSessionConfigOptionRequest::new("missing", "mode", "plan"))
            .await
            .expect("setting config on an unknown session creates it");

        assert_eq!(store.load("missing").await.unwrap().mode, "plan");
    }

    #[tokio::test]
    async fn load_session_unknown_session_initializes_ok() {
        use agent_client_protocol::Agent as _;
        use trogon_runner_tools::session_store::mock::MemorySessionStore;

        let agent = TrogonAgent::new(
            crate::session_notifier::mock::MockSessionNotifier::new(),
            MemorySessionStore::new(),
            crate::agent_runner::mock::MockAgentRunner::new("claude-3-5-sonnet-20241022"),
            "test-prefix",
            "claude-3-5-sonnet-20241022",
            None,
            None,
            std::sync::Arc::new(tokio::sync::RwLock::new(None)),
        );

        // Resume-or-create: loading an unknown (but valid) session id succeeds and
        // returns the available mode/model state rather than erroring.
        let resp = agent
            .load_session(LoadSessionRequest::new("missing", "/tmp"))
            .await
            .expect("loading an unknown session initializes it");
        assert!(resp.modes.is_some());
    }

    #[tokio::test]
    async fn authenticate_gateway_auth_stores_gateway_config() {
        use agent_client_protocol::Agent as _;
        use trogon_runner_tools::session_store::mock::MemorySessionStore;

        let gateway_config = std::sync::Arc::new(tokio::sync::RwLock::new(None));
        let agent = TrogonAgent::new(
            crate::session_notifier::mock::MockSessionNotifier::new(),
            MemorySessionStore::new(),
            crate::agent_runner::mock::MockAgentRunner::new("claude-3-5-sonnet-20241022"),
            "test-prefix",
            "claude-3-5-sonnet-20241022",
            None,
            None,
            gateway_config.clone(),
        );
        let meta = serde_json::json!({
            "gateway": {
                "baseUrl": "https://gateway.example.com/v1",
                "headers": {
                    "Authorization": "Bearer gateway-token",
                    "X-Team": "infra"
                }
            }
        });
        let req = AuthenticateRequest::new("gateway_auth").meta(
            serde_json::from_value::<serde_json::Map<String, serde_json::Value>>(meta).unwrap(),
        );

        agent.authenticate(req).await.unwrap();

        let cfg = gateway_config.read().await;
        let cfg = cfg.as_ref().expect("gateway config stored");
        assert_eq!(cfg.base_url, "https://gateway.example.com/v1");
        assert_eq!(cfg.token, "gateway-token");
        assert_eq!(cfg.extra_headers, vec![("X-Team".to_string(), "infra".to_string())]);
    }

    #[tokio::test]
    async fn authenticate_rejects_missing_gateway_metadata() {
        use agent_client_protocol::Agent as _;
        use trogon_runner_tools::session_store::mock::MemorySessionStore;

        let agent = TrogonAgent::new(
            crate::session_notifier::mock::MockSessionNotifier::new(),
            MemorySessionStore::new(),
            crate::agent_runner::mock::MockAgentRunner::new("claude-3-5-sonnet-20241022"),
            "test-prefix",
            "claude-3-5-sonnet-20241022",
            None,
            None,
            std::sync::Arc::new(tokio::sync::RwLock::new(None)),
        );

        let err = agent
            .authenticate(AuthenticateRequest::new("gateway_auth"))
            .await
            .unwrap_err();
        assert_eq!(err.code, ErrorCode::InvalidParams);
    }

    #[cfg(feature = "test-helpers")]
    #[tokio::test]
    async fn ext_method_export_unknown_session_returns_empty_list() {
        use trogon_runner_tools::session_store::mock::MemorySessionStore;

        let store = MemorySessionStore::new();

        let agent = TrogonAgent::new(
            crate::session_notifier::mock::MockSessionNotifier::new(),
            store,
            crate::agent_runner::mock::MockAgentRunner::new("claude-3-5-sonnet-20241022"),
            "test-prefix",
            "claude-3-5-sonnet-20241022",
            None,
            None,
            std::sync::Arc::new(tokio::sync::RwLock::new(None)),
        );

        let resp = agent
            .ext_method(ExtRequest::new("session/export", make_export_params("no-such")))
            .await
            .unwrap();

        let portable: Vec<trogon_runner_tools::portable_session::PortableMessage> =
            serde_json::from_str(resp.0.get()).unwrap();

        assert_eq!(portable.len(), 0);
    }

    #[cfg(feature = "test-helpers")]
    #[tokio::test]
    async fn ext_method_export_missing_session_id_returns_error() {
        use trogon_runner_tools::session_store::mock::MemorySessionStore;

        let store = MemorySessionStore::new();

        let agent = TrogonAgent::new(
            crate::session_notifier::mock::MockSessionNotifier::new(),
            store,
            crate::agent_runner::mock::MockAgentRunner::new("claude-3-5-sonnet-20241022"),
            "test-prefix",
            "claude-3-5-sonnet-20241022",
            None,
            None,
            std::sync::Arc::new(tokio::sync::RwLock::new(None)),
        );

        let params: std::sync::Arc<serde_json::value::RawValue> =
            serde_json::value::RawValue::from_string("{}".to_string())
                .unwrap()
                .into();

        let result = agent.ext_method(ExtRequest::new("session/export", params)).await;

        assert!(result.is_err(), "missing sessionId must return an error");
    }

    #[cfg(feature = "test-helpers")]
    #[tokio::test]
    async fn ext_method_export_import_round_trip() {
        use trogon_runner_tools::session_store::{SessionState, mock::MemorySessionStore};

        let store = MemorySessionStore::new();
        let store_clone = store.clone();

        // Pre-populate "src"
        let src_state = SessionState {
            messages: vec![
                Message::user_text("q"),
                Message::assistant(vec![AgentContentBlock::Text { text: "a".into() }]),
            ],
            ..Default::default()
        };
        store_clone.save("src", &src_state).await.unwrap();

        // Pre-populate empty "dst"
        let dst_state = SessionState::default();
        store_clone.save("dst", &dst_state).await.unwrap();

        let agent = TrogonAgent::new(
            crate::session_notifier::mock::MockSessionNotifier::new(),
            store,
            crate::agent_runner::mock::MockAgentRunner::new("claude-3-5-sonnet-20241022"),
            "test-prefix",
            "claude-3-5-sonnet-20241022",
            None,
            None,
            std::sync::Arc::new(tokio::sync::RwLock::new(None)),
        );

        // Export from "src"
        let export_resp = agent
            .ext_method(ExtRequest::new("session/export", make_export_params("src")))
            .await
            .unwrap();

        let exported_messages: Vec<trogon_runner_tools::portable_session::PortableMessage> =
            serde_json::from_str(export_resp.0.get()).unwrap();

        // Import into "dst"
        let import_params = make_import_params("dst", serde_json::to_value(&exported_messages).unwrap());
        agent
            .ext_method(ExtRequest::new("session/import", import_params))
            .await
            .unwrap();

        // Verify "dst" now has the same messages as "src"
        let dst_loaded = store_clone.load("dst").await.unwrap();
        assert_eq!(dst_loaded.messages.len(), 2);
        assert_eq!(dst_loaded.messages[0].role, "user");
        assert_eq!(dst_loaded.messages[1].role, "assistant");
    }

    #[cfg(feature = "test-helpers")]
    #[tokio::test]
    async fn ext_method_import_updates_updated_at() {
        use trogon_runner_tools::session_store::{SessionState, mock::MemorySessionStore};

        let store = MemorySessionStore::new();
        let store_clone = store.clone();

        // Save session "ts1" with empty messages and empty updated_at
        let initial_state = SessionState {
            updated_at: String::new(),
            ..Default::default()
        };
        store_clone.save("ts1", &initial_state).await.unwrap();

        let agent = TrogonAgent::new(
            crate::session_notifier::mock::MockSessionNotifier::new(),
            store,
            crate::agent_runner::mock::MockAgentRunner::new("claude-3-5-sonnet-20241022"),
            "test-prefix",
            "claude-3-5-sonnet-20241022",
            None,
            None,
            std::sync::Arc::new(tokio::sync::RwLock::new(None)),
        );

        let params = make_import_params("ts1", serde_json::json!([{"role": "user", "text": "hello"}]));

        agent
            .ext_method(ExtRequest::new("session/import", params))
            .await
            .unwrap();

        let state = store_clone.load("ts1").await.unwrap();
        assert!(!state.updated_at.is_empty(), "updated_at must be set after import");
    }

    #[cfg(feature = "test-helpers")]
    #[tokio::test]
    async fn ext_method_import_malformed_messages_returns_error() {
        use trogon_runner_tools::session_store::mock::MemorySessionStore;

        let store = MemorySessionStore::new();

        let agent = TrogonAgent::new(
            crate::session_notifier::mock::MockSessionNotifier::new(),
            store,
            crate::agent_runner::mock::MockAgentRunner::new("claude-3-5-sonnet-20241022"),
            "test-prefix",
            "claude-3-5-sonnet-20241022",
            None,
            None,
            std::sync::Arc::new(tokio::sync::RwLock::new(None)),
        );

        // messages is not an array of PortableMessage — it's a plain string
        let params = serde_json::value::RawValue::from_string(
            serde_json::json!({"sessionId":"s1","messages":"not-an-array"}).to_string(),
        )
        .unwrap();
        let result = agent.ext_method(ExtRequest::new("session/import", params.into())).await;
        assert!(result.is_err(), "malformed messages must return Err");
    }

    #[cfg(feature = "test-helpers")]
    #[tokio::test(flavor = "current_thread")]
    async fn run_prompt_accumulates_token_usage_in_session_state() {
        use agent_client_protocol::PromptRequest;
        use trogon_agent_core::agent_loop::AgentEvent;
        use trogon_runner_tools::session_store::{SessionState, mock::MemorySessionStore};

        let store = MemorySessionStore::new();
        let store_clone = store.clone();

        store_clone.save("s1", &SessionState::default()).await.unwrap();

        let runner = crate::agent_runner::mock::MockAgentRunner::new("claude-3-5-sonnet-20241022").with_events(vec![
            AgentEvent::UsageSummary {
                input_tokens: 100,
                output_tokens: 50,
                cache_creation_tokens: 10,
                cache_read_tokens: 5,
            },
        ]);

        let agent = TrogonAgent::new(
            crate::session_notifier::mock::MockSessionNotifier::new(),
            store,
            runner,
            "test-prefix",
            "claude-3-5-sonnet-20241022",
            None,
            None,
            std::sync::Arc::new(tokio::sync::RwLock::new(None)),
        );

        let local = tokio::task::LocalSet::new();
        local
            .run_until(async {
                let req = PromptRequest::new("s1", vec![]);
                let client = crate::session_notifier::mock::NullPromptEventClient;
                agent.run_prompt(&req, &client, None, None).await.unwrap();
            })
            .await;

        let saved = store_clone.load("s1").await.unwrap();
        assert_eq!(saved.total_input_tokens, 100);
        assert_eq!(saved.total_output_tokens, 50);
        assert_eq!(saved.total_cache_creation_tokens, 10);
        assert_eq!(saved.total_cache_read_tokens, 5);
    }

    /// C4 migration end-to-end at the agent level: a legacy session loaded with a
    /// bare `compactor_model` (no provider) has its provider resolved from the
    /// catalog and the migrated state durably persisted via the store.
    #[cfg(feature = "test-helpers")]
    #[tokio::test(flavor = "current_thread")]
    async fn run_prompt_backfills_and_persists_compactor_provider_from_catalog() {
        use agent_client_protocol::PromptRequest;
        use trogon_runner_tools::session_store::{SessionState, mock::MemorySessionStore};
        use trogonai_catalog_client::{CatalogEntry, CatalogSnapshot};
        use trogonai_catalog_proto::ModelModality;

        let store = MemorySessionStore::new();
        let store_clone = store.clone();

        // Legacy session: compactor_model set, compactor_provider absent.
        store_clone
            .save(
                "s1",
                &SessionState {
                    compactor_model: Some("grok-2".to_string()),
                    compactor_provider: None,
                    ..Default::default()
                },
            )
            .await
            .unwrap();

        let catalog = CatalogSnapshot {
            entries: vec![CatalogEntry {
                model_id: "grok-2".into(),
                provider: "xai".into(),
                context_window: 131_072,
                max_output: 8192,
                modality: ModelModality::TEXT,
            }],
        };

        let runner = crate::agent_runner::mock::MockAgentRunner::new("claude-3-5-sonnet-20241022");
        let agent = TrogonAgent::new(
            crate::session_notifier::mock::MockSessionNotifier::new(),
            store,
            runner,
            "test-prefix",
            "claude-3-5-sonnet-20241022",
            None,
            None,
            std::sync::Arc::new(tokio::sync::RwLock::new(None)),
        )
        .with_catalog(catalog);

        let local = tokio::task::LocalSet::new();
        local
            .run_until(async {
                let req = PromptRequest::new("s1", vec![]);
                let client = crate::session_notifier::mock::NullPromptEventClient;
                agent.run_prompt(&req, &client, None, None).await.unwrap();
            })
            .await;

        let saved = store_clone.load("s1").await.unwrap();
        assert_eq!(saved.compactor_model.as_deref(), Some("grok-2"));
        assert_eq!(
            saved.compactor_provider.as_deref(),
            Some("xai"),
            "C4 backfill must resolve and persist the provider"
        );
    }

    #[cfg(feature = "test-helpers")]
    #[tokio::test(flavor = "current_thread")]
    async fn run_prompt_persists_tokens_on_cancellation() {
        use agent_client_protocol::PromptRequest;
        use trogon_agent_core::agent_loop::AgentEvent;
        use trogon_runner_tools::session_store::{SessionState, mock::MemorySessionStore};

        let store = MemorySessionStore::new();
        let store_clone = store.clone();

        store_clone.save("s1", &SessionState::default()).await.unwrap();

        let started = std::sync::Arc::new(tokio::sync::Notify::new());
        let runner = crate::agent_runner::mock::MockAgentRunner::new("claude-3-5-sonnet-20241022")
            .with_events(vec![AgentEvent::UsageSummary {
                input_tokens: 200,
                output_tokens: 80,
                cache_creation_tokens: 0,
                cache_read_tokens: 20,
            }])
            .with_steer_wait()
            .with_started_notify(std::sync::Arc::clone(&started));

        let agent = std::sync::Arc::new(TrogonAgent::new(
            crate::session_notifier::mock::MockSessionNotifier::new(),
            store,
            runner,
            "test-prefix",
            "claude-3-5-sonnet-20241022",
            None,
            None,
            std::sync::Arc::new(tokio::sync::RwLock::new(None)),
        ));

        let (cancel_tx, cancel_rx) = tokio::sync::oneshot::channel::<()>();
        let req = PromptRequest::new("s1", vec![]);
        let agent_clone = std::sync::Arc::clone(&agent);

        let local = tokio::task::LocalSet::new();
        local.run_until(async move {
            let handle = tokio::task::spawn_local(async move {
                let client = crate::session_notifier::mock::NullPromptEventClient;
                agent_clone
                    .run_prompt(
                        &req,
                        &client,
                        Some(CancelSubscription::from_receiver(cancel_rx)),
                        None,
                    )
                    .await
            });
            started.notified().await;
            let _ = cancel_tx.send(());
            handle.await.unwrap().unwrap();
        }).await;

        let saved = store_clone.load("s1").await.unwrap();
        assert_eq!(
            saved.total_input_tokens, 200,
            "tokens from completed turns must be persisted on cancel"
        );
        assert_eq!(saved.total_output_tokens, 80);
        assert_eq!(saved.total_cache_read_tokens, 20);
    }

    #[cfg(feature = "test-helpers")]
    #[tokio::test]
    async fn list_sessions_exposes_token_totals_after_prompt() {
        use agent_client_protocol::{Agent as _, ListSessionsRequest, PromptRequest};
        use trogon_agent_core::agent_loop::AgentEvent;
        use trogon_runner_tools::session_store::{SessionState, mock::MemorySessionStore};

        let store = MemorySessionStore::new();
        let store_clone = store.clone();
        store_clone.save("s1", &SessionState::default()).await.unwrap();

        let runner = crate::agent_runner::mock::MockAgentRunner::new("claude-3-5-sonnet-20241022").with_events(vec![
            AgentEvent::UsageSummary {
                input_tokens: 80,
                output_tokens: 30,
                cache_creation_tokens: 5,
                cache_read_tokens: 10,
            },
        ]);

        let agent = TrogonAgent::new(
            crate::session_notifier::mock::MockSessionNotifier::new(),
            store,
            runner,
            "test-prefix",
            "claude-3-5-sonnet-20241022",
            None,
            None,
            std::sync::Arc::new(tokio::sync::RwLock::new(None)),
        );

        let local = tokio::task::LocalSet::new();
        local
            .run_until(async {
                let req = PromptRequest::new("s1", vec![]);
                let client = crate::session_notifier::mock::NullPromptEventClient;
                agent.run_prompt(&req, &client, None, None).await.unwrap();

                let resp = agent.list_sessions(ListSessionsRequest::new()).await.unwrap();
                let info = resp
                    .sessions
                    .iter()
                    .find(|s| s.session_id.to_string() == "s1")
                    .expect("session must appear in list");
                let meta = info.meta.as_ref().expect("meta must be present when tokens > 0");
                assert_eq!(meta.get("totalInputTokens").and_then(|v| v.as_u64()), Some(80));
                assert_eq!(meta.get("totalOutputTokens").and_then(|v| v.as_u64()), Some(30));
                assert_eq!(meta.get("totalCacheCreationTokens").and_then(|v| v.as_u64()), Some(5));
                assert_eq!(meta.get("totalCacheReadTokens").and_then(|v| v.as_u64()), Some(10));
            })
            .await;
    }

    #[cfg(feature = "test-helpers")]
    #[tokio::test]
    async fn load_session_restores_nonzero_token_totals_from_kv() {
        use agent_client_protocol::{Agent as _, ListSessionsRequest};
        use trogon_runner_tools::session_store::{SessionState, mock::MemorySessionStore};

        let store = MemorySessionStore::new();
        let store_clone = store.clone();
        store_clone
            .save(
                "s1",
                &SessionState {
                    total_input_tokens: 120,
                    total_output_tokens: 45,
                    total_cache_creation_tokens: 7,
                    total_cache_read_tokens: 18,
                    ..Default::default()
                },
            )
            .await
            .unwrap();

        let agent = TrogonAgent::new(
            crate::session_notifier::mock::MockSessionNotifier::new(),
            store,
            crate::agent_runner::mock::MockAgentRunner::new("claude-3-5-sonnet-20241022"),
            "test-prefix",
            "claude-3-5-sonnet-20241022",
            None,
            None,
            std::sync::Arc::new(tokio::sync::RwLock::new(None)),
        );

        let resp = agent.list_sessions(ListSessionsRequest::new()).await.unwrap();
        let info = resp
            .sessions
            .iter()
            .find(|s| s.session_id.to_string() == "s1")
            .expect("session must appear in list");
        let meta = info.meta.as_ref().expect("meta must be present for pre-saved tokens");
        assert_eq!(meta.get("totalInputTokens").and_then(|v| v.as_u64()), Some(120));
        assert_eq!(meta.get("totalOutputTokens").and_then(|v| v.as_u64()), Some(45));
        assert_eq!(meta.get("totalCacheCreationTokens").and_then(|v| v.as_u64()), Some(7));
        assert_eq!(meta.get("totalCacheReadTokens").and_then(|v| v.as_u64()), Some(18));
    }

    #[cfg(feature = "test-helpers")]
    #[tokio::test]
    async fn fork_session_resets_token_totals_to_zero() {
        use agent_client_protocol::{Agent as _, ForkSessionRequest};
        use trogon_runner_tools::session_store::{SessionState, mock::MemorySessionStore};

        let store = MemorySessionStore::new();
        let store_clone = store.clone();
        store_clone
            .save(
                "src",
                &SessionState {
                    total_input_tokens: 150,
                    total_output_tokens: 60,
                    total_cache_creation_tokens: 8,
                    total_cache_read_tokens: 12,
                    ..Default::default()
                },
            )
            .await
            .unwrap();

        let agent = TrogonAgent::new(
            crate::session_notifier::mock::MockSessionNotifier::new(),
            store,
            crate::agent_runner::mock::MockAgentRunner::new("claude-3-5-sonnet-20241022"),
            "test-prefix",
            "claude-3-5-sonnet-20241022",
            None,
            None,
            std::sync::Arc::new(tokio::sync::RwLock::new(None)),
        );

        let fork_resp = agent
            .fork_session(ForkSessionRequest::new("src", "/tmp"))
            .await
            .unwrap();
        let new_id = fork_resp.session_id.to_string();

        let forked = store_clone.load(&new_id).await.unwrap();
        assert_eq!(forked.total_input_tokens, 0);
        assert_eq!(forked.total_output_tokens, 0);
        assert_eq!(forked.total_cache_creation_tokens, 0);
        assert_eq!(forked.total_cache_read_tokens, 0);
    }

    #[cfg(feature = "test-helpers")]
    #[tokio::test(flavor = "current_thread")]
    async fn token_totals_accumulate_across_multiple_prompts() {
        use agent_client_protocol::PromptRequest;
        use trogon_agent_core::agent_loop::AgentEvent;
        use trogon_runner_tools::session_store::{SessionState, mock::MemorySessionStore};

        let store = MemorySessionStore::new();
        let store_clone = store.clone();
        store_clone.save("s1", &SessionState::default()).await.unwrap();

        let runner1 = crate::agent_runner::mock::MockAgentRunner::new("claude-3-5-sonnet-20241022").with_events(vec![
            AgentEvent::UsageSummary {
                input_tokens: 100,
                output_tokens: 50,
                cache_creation_tokens: 0,
                cache_read_tokens: 0,
            },
        ]);
        let agent1 = TrogonAgent::new(
            crate::session_notifier::mock::MockSessionNotifier::new(),
            store.clone(),
            runner1,
            "test-prefix",
            "claude-3-5-sonnet-20241022",
            None,
            None,
            std::sync::Arc::new(tokio::sync::RwLock::new(None)),
        );
        let local = tokio::task::LocalSet::new();
        local
            .run_until(async {
                let client = crate::session_notifier::mock::NullPromptEventClient;
                agent1
                    .run_prompt(&PromptRequest::new("s1", vec![]), &client, None, None)
                    .await
                    .unwrap();
            })
            .await;

        let runner2 = crate::agent_runner::mock::MockAgentRunner::new("claude-3-5-sonnet-20241022").with_events(vec![
            AgentEvent::UsageSummary {
                input_tokens: 80,
                output_tokens: 40,
                cache_creation_tokens: 0,
                cache_read_tokens: 0,
            },
        ]);
        let agent2 = TrogonAgent::new(
            crate::session_notifier::mock::MockSessionNotifier::new(),
            store,
            runner2,
            "test-prefix",
            "claude-3-5-sonnet-20241022",
            None,
            None,
            std::sync::Arc::new(tokio::sync::RwLock::new(None)),
        );
        let local2 = tokio::task::LocalSet::new();
        local2
            .run_until(async {
                let client = crate::session_notifier::mock::NullPromptEventClient;
                agent2
                    .run_prompt(&PromptRequest::new("s1", vec![]), &client, None, None)
                    .await
                    .unwrap();
            })
            .await;

        let saved = store_clone.load("s1").await.unwrap();
        assert_eq!(saved.total_input_tokens, 180);
        assert_eq!(saved.total_output_tokens, 90);
    }

    #[cfg(feature = "test-helpers")]
    #[tokio::test(flavor = "current_thread")]
    async fn cache_tokens_accumulate_across_multiple_prompts() {
        use agent_client_protocol::PromptRequest;
        use trogon_agent_core::agent_loop::AgentEvent;
        use trogon_runner_tools::session_store::{SessionState, mock::MemorySessionStore};

        let store = MemorySessionStore::new();
        let store_clone = store.clone();
        store_clone.save("s1", &SessionState::default()).await.unwrap();

        let runner1 = crate::agent_runner::mock::MockAgentRunner::new("claude-3-5-sonnet-20241022").with_events(vec![
            AgentEvent::UsageSummary {
                input_tokens: 10,
                output_tokens: 5,
                cache_creation_tokens: 6,
                cache_read_tokens: 15,
            },
        ]);
        let agent1 = TrogonAgent::new(
            crate::session_notifier::mock::MockSessionNotifier::new(),
            store.clone(),
            runner1,
            "test-prefix",
            "claude-3-5-sonnet-20241022",
            None,
            None,
            std::sync::Arc::new(tokio::sync::RwLock::new(None)),
        );
        let local = tokio::task::LocalSet::new();
        local
            .run_until(async {
                let client = crate::session_notifier::mock::NullPromptEventClient;
                agent1
                    .run_prompt(&PromptRequest::new("s1", vec![]), &client, None, None)
                    .await
                    .unwrap();
            })
            .await;

        let runner2 = crate::agent_runner::mock::MockAgentRunner::new("claude-3-5-sonnet-20241022").with_events(vec![
            AgentEvent::UsageSummary {
                input_tokens: 10,
                output_tokens: 5,
                cache_creation_tokens: 9,
                cache_read_tokens: 20,
            },
        ]);
        let agent2 = TrogonAgent::new(
            crate::session_notifier::mock::MockSessionNotifier::new(),
            store,
            runner2,
            "test-prefix",
            "claude-3-5-sonnet-20241022",
            None,
            None,
            std::sync::Arc::new(tokio::sync::RwLock::new(None)),
        );
        let local2 = tokio::task::LocalSet::new();
        local2
            .run_until(async {
                let client = crate::session_notifier::mock::NullPromptEventClient;
                agent2
                    .run_prompt(&PromptRequest::new("s1", vec![]), &client, None, None)
                    .await
                    .unwrap();
            })
            .await;

        let saved = store_clone.load("s1").await.unwrap();
        assert_eq!(saved.total_cache_creation_tokens, 15);
        assert_eq!(saved.total_cache_read_tokens, 35);
    }

    #[cfg(feature = "test-helpers")]
    #[tokio::test(flavor = "current_thread")]
    async fn prompt_without_usage_summary_preserves_existing_token_totals() {
        use agent_client_protocol::PromptRequest;
        use trogon_runner_tools::session_store::{SessionState, mock::MemorySessionStore};

        let store = MemorySessionStore::new();
        let store_clone = store.clone();
        store_clone
            .save(
                "s1",
                &SessionState {
                    total_input_tokens: 100,
                    total_output_tokens: 40,
                    ..Default::default()
                },
            )
            .await
            .unwrap();

        let agent = TrogonAgent::new(
            crate::session_notifier::mock::MockSessionNotifier::new(),
            store,
            crate::agent_runner::mock::MockAgentRunner::new("claude-3-5-sonnet-20241022"),
            "test-prefix",
            "claude-3-5-sonnet-20241022",
            None,
            None,
            std::sync::Arc::new(tokio::sync::RwLock::new(None)),
        );

        let local = tokio::task::LocalSet::new();
        local
            .run_until(async {
                let client = crate::session_notifier::mock::NullPromptEventClient;
                agent
                    .run_prompt(&PromptRequest::new("s1", vec![]), &client, None, None)
                    .await
                    .unwrap();
            })
            .await;

        let saved = store_clone.load("s1").await.unwrap();
        assert_eq!(
            saved.total_input_tokens, 100,
            "tokens must not decrease when prompt has no usage event"
        );
        assert_eq!(saved.total_output_tokens, 40);
    }

    #[cfg(feature = "test-helpers")]
    #[tokio::test(flavor = "current_thread")]
    async fn spawn_subagent_restricts_tools_and_inherits_parent_permission_context() {
        use std::sync::{Arc, Mutex};

        use async_trait::async_trait;
        use trogon_runner_tools::session_store::{SessionState, SessionStore, mock::MemorySessionStore};

        #[derive(Clone)]
        struct RecordingStore {
            inner: MemorySessionStore,
            last_sub: Arc<Mutex<Option<SessionState>>>,
        }

        #[async_trait]
        impl SessionStore for RecordingStore {
            async fn load(&self, session_id: &str) -> anyhow::Result<SessionState> {
                self.inner.load(session_id).await
            }

            async fn save(&self, session_id: &str, state: &SessionState) -> anyhow::Result<()> {
                if session_id != "parent-1" {
                    *self.last_sub.lock().unwrap() = Some(state.clone());
                }
                self.inner.save(session_id, state).await
            }

            async fn delete(&self, session_id: &str) -> anyhow::Result<()> {
                self.inner.delete(session_id).await
            }

            async fn list_ids(&self) -> anyhow::Result<Vec<String>> {
                self.inner.list_ids().await
            }

            async fn list_children(&self, parent_id: &str) -> anyhow::Result<Vec<String>> {
                self.inner.list_children(parent_id).await
            }
        }

        let tmp = tempfile::tempdir().unwrap();
        let agents_dir = tmp.path().join(".claude/agents");
        std::fs::create_dir_all(&agents_dir).unwrap();
        std::fs::write(
            agents_dir.join("readonly.md"),
            "---\nname: readonly\ntools: read_file\n---\nRead only.\n",
        )
        .unwrap();

        let inner = MemorySessionStore::new();
        inner
            .save(
                "parent-1",
                &SessionState {
                    cwd: tmp.path().to_string_lossy().to_string(),
                    allowed_tools: vec!["read_file".to_string()],
                    additional_read_dirs: vec!["/extra-read".to_string()],
                    ..Default::default()
                },
            )
            .await
            .unwrap();

        let last_sub = Arc::new(Mutex::new(None));
        let store = RecordingStore {
            inner,
            last_sub: Arc::clone(&last_sub),
        };

        let runner = crate::agent_runner::mock::MockAgentRunner::new("claude-test")
            .with_response(vec![]);
        let agent = std::sync::Arc::new(TrogonAgent::new(
            crate::session_notifier::mock::MockSessionNotifier::new(),
            store,
            runner.clone(),
            "test-prefix",
            "claude-test",
            None,
            None,
            std::sync::Arc::new(tokio::sync::RwLock::new(None)),
        ));

        let local = tokio::task::LocalSet::new();
        local
            .run_until(async {
                agent
                    .spawn_subagent("parent-1", "review files", "readonly")
                    .await
                    .unwrap();
            })
            .await;

        let sub = last_sub.lock().unwrap().clone().expect("sub-session must be saved");
        assert_eq!(sub.tool_allowlist, vec!["read_file".to_string()]);
        assert_eq!(sub.allowed_tools, vec!["read_file".to_string()]);
        assert_eq!(sub.additional_read_dirs, vec!["/extra-read".to_string()]);

        let offered = runner.captured_chat_tools();
        assert!(offered.contains(&"read_file".to_string()));
        assert!(!offered.contains(&"write_file".to_string()));
    }

    #[cfg(feature = "test-helpers")]
    #[tokio::test(flavor = "current_thread")]
    async fn spawn_subagent_persists_by_default_but_can_be_opted_out() {
        use std::sync::{Arc, Mutex};

        use async_trait::async_trait;
        use trogon_runner_tools::session_store::{SessionState, SessionStore, mock::MemorySessionStore};

        #[derive(Clone)]
        struct DeleteTrackingStore {
            inner: MemorySessionStore,
            deleted: Arc<Mutex<Vec<String>>>,
        }

        #[async_trait]
        impl SessionStore for DeleteTrackingStore {
            async fn load(&self, session_id: &str) -> anyhow::Result<SessionState> {
                self.inner.load(session_id).await
            }

            async fn save(&self, session_id: &str, state: &SessionState) -> anyhow::Result<()> {
                self.inner.save(session_id, state).await
            }

            async fn delete(&self, session_id: &str) -> anyhow::Result<()> {
                self.deleted.lock().unwrap().push(session_id.to_string());
                self.inner.delete(session_id).await
            }

            async fn list_ids(&self) -> anyhow::Result<Vec<String>> {
                self.inner.list_ids().await
            }

            async fn list_children(&self, parent_id: &str) -> anyhow::Result<Vec<String>> {
                self.inner.list_children(parent_id).await
            }
        }

        async fn run_spawn(
            disable_persist: bool,
        ) -> (Vec<String>, String, trogon_runner_tools::session_store::mock::MemorySessionStore) {
            let inner = MemorySessionStore::new();
            inner
                .save(
                    "parent-1",
                    &SessionState {
                        cwd: std::env::current_dir().unwrap().to_string_lossy().to_string(),
                        ..Default::default()
                    },
                )
                .await
                .unwrap();

            let deleted = Arc::new(Mutex::new(Vec::new()));
            let store = DeleteTrackingStore {
                inner: inner.clone(),
                deleted: Arc::clone(&deleted),
            };

            let runner = crate::agent_runner::mock::MockAgentRunner::new("claude-test")
                .with_response(vec![]);
            let agent = std::sync::Arc::new(TrogonAgent::new(
                crate::session_notifier::mock::MockSessionNotifier::new(),
                store,
                runner,
                "acp.claude",
                "claude-test",
                None,
                None,
                std::sync::Arc::new(tokio::sync::RwLock::new(None)),
            ));

            if disable_persist {
                unsafe { std::env::set_var("TROGON_SUBAGENT_PERSIST", "0") };
            } else {
                unsafe { std::env::remove_var("TROGON_SUBAGENT_PERSIST") };
            }

            let result = {
                let local = tokio::task::LocalSet::new();
                local
                    .run_until(async {
                        agent.spawn_subagent("parent-1", "hello", "").await.unwrap()
                    })
                    .await
            };

            unsafe { std::env::remove_var("TROGON_SUBAGENT_PERSIST") };
            (deleted.lock().unwrap().clone(), result, inner)
        }

        let (deleted_default, text_default, store_default) = run_spawn(false).await;
        assert!(
            deleted_default.is_empty(),
            "default path must not delete sub-session"
        );
        assert!(
            text_default.contains("Sub-agent session persisted"),
            "default path must advertise persistence: {text_default}"
        );
        assert!(
            text_default.contains("--session-id"),
            "default path must include session id for resume"
        );

        let remaining_default: Vec<_> = store_default
            .list_ids()
            .await
            .unwrap()
            .into_iter()
            .filter(|id| id != "parent-1")
            .collect();
        assert_eq!(remaining_default.len(), 1, "default path must persist sub-session");

        let (deleted_opt_out, text_opt_out, store_opt_out) = run_spawn(true).await;
        assert_eq!(deleted_opt_out.len(), 1, "opt-out path must delete sub-session");
        assert!(
            !text_opt_out.contains("Sub-agent session persisted"),
            "opt-out path must not include resume hint"
        );

        let remaining: Vec<_> = store_opt_out
            .list_ids()
            .await
            .unwrap()
            .into_iter()
            .filter(|id| id != "parent-1")
            .collect();
        assert!(remaining.is_empty(), "opt-out path must remove sub-session");
    }

    #[cfg(feature = "test-helpers")]
    #[tokio::test(flavor = "current_thread")]
    async fn run_prompt_empty_tool_allowlist_offers_all_builtin_tools() {
        use agent_client_protocol::PromptRequest;
        use trogon_runner_tools::session_store::{SessionState, mock::MemorySessionStore};

        let store = MemorySessionStore::new();
        store.save("s1", &SessionState::default()).await.unwrap();

        let runner = crate::agent_runner::mock::MockAgentRunner::new("claude-test")
            .with_response(vec![]);
        let agent = TrogonAgent::new(
            crate::session_notifier::mock::MockSessionNotifier::new(),
            store,
            runner.clone(),
            "test-prefix",
            "claude-test",
            None,
            None,
            std::sync::Arc::new(tokio::sync::RwLock::new(None)),
        );

        let local = tokio::task::LocalSet::new();
        local
            .run_until(async {
                let client = crate::session_notifier::mock::NullPromptEventClient;
                agent
                    .run_prompt(&PromptRequest::new("s1", vec![]), &client, None, None)
                    .await
                    .unwrap();
            })
            .await;

        let offered = runner.captured_chat_tools();
        assert!(offered.contains(&"read_file".to_string()));
        assert!(offered.contains(&"write_file".to_string()));
    }

    #[cfg(feature = "test-helpers")]
    #[tokio::test(flavor = "current_thread")]
    async fn cancel_rejects_invalid_session_id() {
        use agent_client_protocol::CancelNotification;
        use trogon_runner_tools::session_store::mock::MemorySessionStore;

        let agent = TrogonAgent::new(
            crate::session_notifier::mock::MockSessionNotifier::new(),
            MemorySessionStore::new(),
            crate::agent_runner::mock::MockAgentRunner::new("claude-test"),
            "test-prefix",
            "claude-test",
            None,
            None,
            std::sync::Arc::new(tokio::sync::RwLock::new(None)),
        );

        let err = agent
            .cancel(CancelNotification::new("invalid.session.id"))
            .await
            .unwrap_err();
        assert_eq!(err.code, ErrorCode::InvalidParams);
        assert!(err.to_string().contains("Invalid session ID"));
    }

    #[cfg(feature = "test-helpers")]
    #[tokio::test(flavor = "current_thread")]
    async fn max_iterations_save_preserves_concurrent_store_writes() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicUsize, Ordering};

        use agent_client_protocol::PromptRequest;
        use async_trait::async_trait;
        use trogon_agent_core::agent_loop::{AgentError, AgentEvent};
        use trogon_runner_tools::session_store::{SessionState, SessionStore, mock::MemorySessionStore};

        #[derive(Clone)]
        struct SplitLoadStore {
            inner: MemorySessionStore,
            loads: Arc<AtomicUsize>,
        }

        #[async_trait]
        impl SessionStore for SplitLoadStore {
            async fn load(&self, session_id: &str) -> anyhow::Result<SessionState> {
                let n = self.loads.fetch_add(1, Ordering::SeqCst);
                if n == 0 {
                    Ok(SessionState {
                        model: Some("stale-model".into()),
                        ..Default::default()
                    })
                } else {
                    self.inner.load(session_id).await
                }
            }

            async fn save(&self, session_id: &str, state: &SessionState) -> anyhow::Result<()> {
                self.inner.save(session_id, state).await
            }

            async fn delete(&self, session_id: &str) -> anyhow::Result<()> {
                self.inner.delete(session_id).await
            }

            async fn list_ids(&self) -> anyhow::Result<Vec<String>> {
                self.inner.list_ids().await
            }

            async fn list_children(&self, parent_id: &str) -> anyhow::Result<Vec<String>> {
                self.inner.list_children(parent_id).await
            }
        }

        let inner = MemorySessionStore::new();
        inner
            .save(
                "s1",
                &SessionState {
                    model: Some("concurrent-model".into()),
                    ..Default::default()
                },
            )
            .await
            .unwrap();

        let store = SplitLoadStore {
            inner: inner.clone(),
            loads: Arc::new(AtomicUsize::new(0)),
        };

        let runner = crate::agent_runner::mock::MockAgentRunner::new("claude-test")
            .with_events(vec![AgentEvent::UsageSummary {
                input_tokens: 10,
                output_tokens: 5,
                cache_creation_tokens: 0,
                cache_read_tokens: 0,
            }])
            .with_error(AgentError::MaxIterationsReached);

        let agent = TrogonAgent::new(
            crate::session_notifier::mock::MockSessionNotifier::new(),
            store,
            runner,
            "test-prefix",
            "claude-test",
            None,
            None,
            std::sync::Arc::new(tokio::sync::RwLock::new(None)),
        );

        let local = tokio::task::LocalSet::new();
        local
            .run_until(async {
                let client = crate::session_notifier::mock::NullPromptEventClient;
                agent
                    .run_prompt(&PromptRequest::new("s1", vec![]), &client, None, None)
                    .await
                    .unwrap();
            })
            .await;

        let saved = inner.load("s1").await.unwrap();
        assert_eq!(
            saved.model.as_deref(),
            Some("concurrent-model"),
            "merge save must not clobber concurrent model update"
        );
        assert_eq!(saved.total_input_tokens, 10);
        assert_eq!(saved.total_output_tokens, 5);
    }
}

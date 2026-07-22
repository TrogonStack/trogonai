//! `MultiRunnerAgent` — an additive routing layer that wraps `TrogonAcpAgent`.
//!
//! The inner `TrogonAcpAgent` is the unchanged Claude-only agent. This wrapper adds
//! external-runner routing ALONGSIDE it: when a session's model resolves (via the
//! registry) to a runner prefix other than the embedded one, prompts/cancels for that
//! session are routed to a per-runner `Bridge` pool; every other session and every other
//! ACP method delegates to the inner agent unchanged.
//!
//! Nothing in `TrogonAcpAgent` is modified — this is a decorator. The `Clone` bounds the
//! pool needs live here, on a type whose only constructor (in `main.rs`) uses concrete
//! `Clone` types, so no existing test is affected.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use acp_nats::nats::{FlushClient, PublishClient, RequestClient, SubscribeClient};
use acp_nats::{AcpPrefix, AgentHandler, Bridge, Config};
use agent_client_protocol::schema::v1::{
    AuthenticateRequest, AuthenticateResponse, CancelNotification, CloseSessionRequest, CloseSessionResponse,
    ExtNotification, ExtRequest, ExtResponse, ForkSessionRequest, ForkSessionResponse, InitializeRequest,
    InitializeResponse, ListSessionsRequest, ListSessionsResponse, LoadSessionRequest, LoadSessionResponse,
    LogoutRequest, LogoutResponse, NewSessionRequest, NewSessionResponse, PromptRequest, PromptResponse,
    ResumeSessionRequest, ResumeSessionResponse, SessionConfigKind, SessionConfigOption, SessionConfigOptionCategory,
    SessionConfigOptionValue, SessionConfigSelectOption, SessionId, SessionNotification, SetSessionConfigOptionRequest,
    SetSessionConfigOptionResponse, SetSessionModeRequest, SetSessionModeResponse,
};
use agent_client_protocol::{Error, ErrorCode, Result};
use tokio::sync::mpsc;
use tracing::warn;
use trogon_acp_runner::{SessionNotifier, SessionStore};
use trogon_cli::CrossRunnerSwitcher;
use trogon_nats::jetstream::{JetStreamGetStream, JetStreamPublisher, JsMessageOf, JsRequestMessage};
use trogon_runner_tools::portable_session::{
    ParsedExport, PortableBlock, PortableMessage, export_json_from_wire, parse_export_json, v1_to_messages,
    v2_to_messages,
};
use trogon_std::time::GetElapsed;
use trogon_tools::{ContentBlock, Message};

use crate::agent::TrogonAcpAgent;

/// runner_sid → acp_sid remap table, shared with the notification relay loop in `main.rs`.
pub type IdRemap = Arc<Mutex<HashMap<String, String>>>;

/// runner_prefix → `Bridge`, created on demand. One bridge per external runner.
type RunnerBridges<N, C, J> = Arc<Mutex<HashMap<String, Arc<Bridge<N, C, J>>>>>;

pub struct MultiRunnerAgent<N, C, J, S, Notif, R = async_nats::jetstream::kv::Store>
where
    N: RequestClient + PublishClient + SubscribeClient + FlushClient,
    C: GetElapsed,
    J: JetStreamPublisher + JetStreamGetStream,
    JsMessageOf<J>: JsRequestMessage,
    S: SessionStore + Clone,
    Notif: SessionNotifier,
    R: trogon_registry::RegistryStore,
{
    /// The unchanged Claude-only agent. Default target for every session/method.
    inner: TrogonAcpAgent<N, C, J, S, Notif>,
    /// A clone of inner's store — used by `sync_session_to_kv` to persist
    /// external-runner history to KV after each prompt, keeping load/fork/list consistent.
    store: S,
    /// Factory inputs cloned to build one `Bridge` per external runner prefix.
    nats: N,
    js: J,
    clock: C,
    base_config: Config,
    /// Shared with each pool `Bridge` so runner notifications reach the IDE channel
    /// (remapped runner_sid → acp_sid by the loop in `main.rs`).
    notification_sender: mpsc::Sender<SessionNotification>,
    /// Discovers which runner prefix owns a given model.
    registry: trogon_registry::Registry<R>,
    /// The embedded (Claude) runner's prefix. Models on this prefix stay on `inner`.
    embedded_prefix: String,
    /// runner_prefix → Bridge (created on demand).
    runner_bridges: RunnerBridges<N, C, J>,
    /// acp_sid → (runner_prefix, runner_sid). Absent ⇒ session served by `inner`.
    active_sessions: Arc<Mutex<HashMap<String, (String, String)>>>,
    /// acp_sid → cwd, captured in `new_session` so a runner session can be opened later.
    session_cwd: Arc<Mutex<HashMap<String, String>>>,
    /// runner_sid → acp_sid, shared with the notification relay in `main.rs`.
    id_remap: IdRemap,
    /// Optional canonical switch orchestrator backed by the Session Kernel. When present
    /// and configured for `runner_binding_mode=canonical`, cross-runner model changes
    /// are driven through the kernel instead of the legacy lossy handoff path. Uses a
    /// `tokio::sync::Mutex` (not `std::sync::Mutex`) because the guard is held across the
    /// `.await` of `switch_model_for_session_into`, which needs `&mut` access for its
    /// duration; `tokio::sync::MutexGuard` is `Send`, so this keeps the enclosing
    /// `AgentHandler` futures `Send`.
    canonical_switcher: Option<Arc<tokio::sync::Mutex<CrossRunnerSwitcher<R>>>>,
}

impl<N, C, J, S, Notif, R> MultiRunnerAgent<N, C, J, S, Notif, R>
where
    N: RequestClient + PublishClient + SubscribeClient + FlushClient + Clone + Send + Sync + 'static,
    C: GetElapsed + Clone + Send + Sync + 'static,
    C::Instant: Send,
    J: JetStreamPublisher + JetStreamGetStream + Clone + Send + Sync + 'static,
    JsMessageOf<J>: JsRequestMessage,
    S: SessionStore + Clone + Send + Sync + 'static,
    Notif: SessionNotifier + Send + Sync + 'static,
    R: trogon_registry::RegistryStore,
{
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        inner: TrogonAcpAgent<N, C, J, S, Notif>,
        store: S,
        nats: N,
        js: J,
        clock: C,
        base_config: Config,
        registry: trogon_registry::Registry<R>,
        notification_sender: mpsc::Sender<SessionNotification>,
        embedded_prefix: impl Into<String>,
    ) -> Self {
        Self {
            inner,
            store,
            nats,
            js,
            clock,
            base_config,
            notification_sender,
            registry,
            embedded_prefix: embedded_prefix.into(),
            runner_bridges: Arc::new(Mutex::new(HashMap::new())),
            active_sessions: Arc::new(Mutex::new(HashMap::new())),
            session_cwd: Arc::new(Mutex::new(HashMap::new())),
            id_remap: Arc::new(Mutex::new(HashMap::new())),
            canonical_switcher: None,
        }
    }

    pub fn with_canonical_switcher(mut self, switcher: CrossRunnerSwitcher<R>) -> Self {
        self.canonical_switcher = Some(Arc::new(tokio::sync::Mutex::new(switcher)));
        self
    }

    /// Clone of the shared remap table, for the notification relay loop in `main.rs`.
    pub fn id_remap_handle(&self) -> IdRemap {
        Arc::clone(&self.id_remap)
    }

    /// Get or create the `Bridge` for an external runner prefix. Returns an `Arc` so the
    /// caller releases the `runner_bridges` lock before any `.await`.
    fn get_or_create_bridge(&self, prefix: &str) -> Option<Arc<Bridge<N, C, J>>> {
        if let Some(b) = self
            .runner_bridges
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(prefix)
        {
            return Some(b.clone());
        }
        let acp_prefix = AcpPrefix::new(prefix).ok()?;
        let config = self.base_config.for_prefix(acp_prefix);
        let meter = opentelemetry::global::meter("trogon-acp-multi-runner");
        let bridge = Arc::new(Bridge::new(
            self.nats.clone(),
            self.js.clone(),
            self.clock.clone(),
            &meter,
            config,
            self.notification_sender.clone(),
        ));
        self.runner_bridges
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(prefix.to_string(), bridge.clone());
        Some(bridge)
    }

    /// Resolve a model id to an EXTERNAL runner prefix. `None` if the model is unknown or
    /// belongs to the embedded (Claude) runner — in which case the session stays on `inner`.
    async fn resolve_external_prefix(&self, model: &str) -> Option<String> {
        let cap = self.registry.find_by_model(model).await.ok()??;
        let prefix = cap.metadata.get("acp_prefix")?.as_str()?.to_string();
        if prefix == self.embedded_prefix {
            None
        } else {
            Some(prefix)
        }
    }

    /// Ensure an external runner session exists for `acp_sid` on `ext_prefix`, opening one
    /// (`new_session`) on first assignment and recording the routing + remap tables.
    /// Export a session's history from a runner via `session/export`.
    /// Returns `None` on failure (network error, runner not found, etc.).
    async fn export_history(&self, prefix: &str, session_id: &str) -> Option<Vec<Message>> {
        let bridge = self.get_or_create_bridge(prefix)?;
        let params =
            serde_json::value::RawValue::from_string(serde_json::json!({ "sessionId": session_id }).to_string())
                .ok()?;
        match bridge.ext_method(ExtRequest::new("session/export", params.into())).await {
            Ok(resp) => {
                // The runner emits wire JSON (V1 array or versioned V2 object) via
                // `export_json_from_wire`; parse it back the same way the runners do.
                // Fall back to a legacy portable JSON array when wire parsing fails.
                match parse_export_json(resp.0.get()) {
                    Ok(ParsedExport::V1(msgs)) => Some(v1_to_messages(&msgs)),
                    Ok(ParsedExport::V2(exp)) => Some(v2_to_messages(&exp)),
                    Err(wire_err) => match serde_json::from_str::<Vec<PortableMessage>>(resp.0.get()) {
                        Ok(portable) => Some(messages_from_portable(&portable)),
                        Err(e) => {
                            warn!(
                                error = %e,
                                wire_error = %wire_err,
                                prefix,
                                "multi-runner: session/export parse failed"
                            );
                            None
                        }
                    },
                }
            },
            Err(e) => {
                warn!(error = %e, prefix, "multi-runner: session/export failed — history not transferred");
                None
            }
        }
    }

    /// Import portable history into a runner session via `session/import`.
    async fn import_history(&self, prefix: &str, session_id: &str, history: &[Message]) {
        let Some(bridge) = self.get_or_create_bridge(prefix) else {
            return;
        };
        // Serialize to the same wire JSON the runner's `session/import` expects
        // (V1 array or versioned V2 object), produced by `export_json_from_wire`.
        // Fall back to a legacy portable JSON array when wire serialization fails.
        let messages_json = match export_json_from_wire(history) {
            Ok(j) => j,
            Err(wire_err) => match serde_json::to_string(&portable_from_messages(history)) {
                Ok(j) => j,
                Err(e) => {
                    warn!(
                        error = %e,
                        wire_error = %wire_err,
                        "multi-runner: failed to serialize history for import"
                    );
                    return;
                }
            },
        };
        let Ok(params) = serde_json::value::RawValue::from_string(format!(
            r#"{{"sessionId":"{session_id}","messages":{messages_json}}}"#
        )) else {
            return;
        };
        if let Err(e) = bridge.ext_method(ExtRequest::new("session/import", params.into())).await {
            warn!(error = %e, prefix, "multi-runner: session/import failed");
        }
    }

    /// Open a fresh session on a runner and set the selected model on it. Returns the
    /// runner's session id. `NewSessionRequest` has no model field, so the model must be
    /// set explicitly — otherwise the runner uses its own default model (e.g. xai would
    /// run grok-4 even if the user picked grok-3-mini).
    async fn open_runner_session(&self, prefix: &str, cwd: &str, model_id: &str) -> Option<String> {
        let bridge = self.get_or_create_bridge(prefix)?;
        let runner_sid = match bridge.new_session(NewSessionRequest::new(PathBuf::from(cwd))).await {
            Ok(r) => r.session_id.0.to_string(),
            Err(e) => {
                warn!(error = %e, prefix, "multi-runner: runner new_session failed — staying on inner");
                return None;
            }
        };
        // Communicate the selected model to the runner (best-effort). Routed through
        // set_session_config_option("model", ...) — agent-client-protocol 1.2.0 removed
        // the dedicated set_session_model wire method entirely (NATS subject
        // `.agent.set_model` is gone; the runner-side equivalent is `.agent.set_config_option`).
        let _ = bridge
            .set_session_config_option(SetSessionConfigOptionRequest::new(
                SessionId::new(runner_sid.clone()),
                "model",
                model_id,
            ))
            .await;
        Some(runner_sid)
    }

    /// Close a runner session to free its memory.
    async fn close_runner_session(&self, prefix: &str, session_id: &str) {
        let Some(bridge) = self.get_or_create_bridge(prefix) else {
            return;
        };
        let _ = bridge
            .close_session(CloseSessionRequest::new(SessionId::new(session_id)))
            .await;
    }

    /// The (prefix, runner_sid) a session is routed to, if any.
    fn route_of(&self, acp_sid: &str) -> Option<(String, String)> {
        self.active_sessions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(acp_sid)
            .cloned()
    }

    /// After each external prompt: export history from the runner and persist it
    /// to KV under `acp_sid`. Keeps `load_session`, `fork_session`, and
    /// `list_sessions` consistent with the actual conversation state.
    async fn post_prompt_sync_kv(&self, acp_sid: &str, prefix: &str, runner_sid: &str) {
        if let Some(history) = self.export_history(prefix, runner_sid).await {
            let _ = self.sync_session_to_kv(acp_sid, &history).await;
        }
    }

    /// Write `history` into KV under `acp_sid.messages`. Best-effort.
    async fn sync_session_to_kv(&self, acp_sid: &str, history: &[Message]) -> anyhow::Result<()> {
        let mut state = self.store.load(acp_sid).await?;
        state.messages = history.to_vec();
        self.store.save(acp_sid, &state).await
    }

    /// Query the registry for the full cross-runner model list and, if any models are
    /// found, overlay them onto the "model" entry of `config_options` — replacing
    /// `TrogonAcpAgent`'s Claude-only select options with the full multi-runner list.
    /// Best-effort: on registry failure or an empty result, `config_options` is returned
    /// unchanged (the inner agent's Claude-only model list stands).
    async fn overlay_registry_models(
        &self,
        config_options: Option<Vec<SessionConfigOption>>,
        current_model: &str,
    ) -> Option<Vec<SessionConfigOption>> {
        let Ok(all) = self.registry.list_all().await else {
            return config_options;
        };
        let mut seen = HashSet::new();
        let model_options: Vec<SessionConfigSelectOption> = all
            .into_iter()
            .flat_map(|cap| {
                cap.metadata
                    .get("models")
                    .and_then(|v| v.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|m| m.as_str())
                            .map(|id| SessionConfigSelectOption::new(id.to_string(), id.to_string()))
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default()
            })
            .filter(|opt| seen.insert(opt.value.0.to_string()))
            .collect();
        if model_options.is_empty() {
            return config_options;
        }
        let model_entry = SessionConfigOption::select("model", "Model", current_model.to_string(), model_options)
            .category(SessionConfigOptionCategory::Model);
        Some(match config_options {
            Some(mut opts) => {
                if let Some(existing) = opts.iter_mut().find(|o| o.id.0.as_ref() == "model") {
                    *existing = model_entry;
                } else {
                    opts.push(model_entry);
                }
                opts
            }
            None => vec![model_entry],
        })
    }

    /// Extract the current model id from a response's "model" config option, if present.
    fn current_model_from_config_options(config_options: &Option<Vec<SessionConfigOption>>) -> Option<String> {
        config_options.as_ref()?.iter().find(|o| o.id.0.as_ref() == "model").and_then(|o| {
            if let SessionConfigKind::Select(s) = &o.kind {
                Some(s.current_value.0.to_string())
            } else {
                None
            }
        })
    }

    #[cfg(test)]
    pub fn session_cwd_snapshot(&self) -> std::collections::HashMap<String, String> {
        self.session_cwd.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone()
    }

    #[cfg(test)]
    pub fn active_sessions_snapshot(&self) -> std::collections::HashMap<String, (String, String)> {
        self.active_sessions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
}

// ── Portable↔Native message converters ──────────────────────────────────────

fn messages_from_portable(history: &[PortableMessage]) -> Vec<Message> {
    history
        .iter()
        .map(|pm| {
            let content = if pm.blocks.is_empty() {
                vec![ContentBlock::Text { text: pm.text.clone() }]
            } else {
                pm.blocks
                    .iter()
                    .filter_map(|b| match b {
                        PortableBlock::Text { text } => Some(ContentBlock::Text { text: text.clone() }),
                        PortableBlock::ToolUse {
                            id,
                            name,
                            input_summary,
                            input,
                            parent_tool_use_id,
                        } => Some(ContentBlock::ToolUse {
                            id: id.clone(),
                            name: name.clone(),
                            // Prefer the full structured `input`; fall back to parsing the
                            // summary for legacy V2 payloads that predate the field.
                            input: if input.is_null() {
                                serde_json::from_str(input_summary)
                                    .unwrap_or_else(|_| serde_json::Value::String(input_summary.clone()))
                            } else {
                                input.clone()
                            },
                            parent_tool_use_id: parent_tool_use_id.clone(),
                        }),
                        PortableBlock::ToolResult { id, output_summary, output } => Some(ContentBlock::ToolResult {
                            tool_use_id: id.clone(),
                            content: output.clone().unwrap_or_else(|| output_summary.clone()),
                            blocks: vec![],
                        }),
                        PortableBlock::Thinking { .. } => None,
                    })
                    .collect()
            };
            Message {
                role: pm.role.clone(),
                content,
            }
        })
        .collect()
}

fn portable_from_messages(messages: &[Message]) -> Vec<PortableMessage> {
    messages
        .iter()
        .map(|m| {
            let blocks: Vec<PortableBlock> = m
                .content
                .iter()
                .filter_map(|b| match b {
                    ContentBlock::Text { text } => Some(PortableBlock::Text { text: text.clone() }),
                    ContentBlock::ToolUse {
                        id,
                        name,
                        input,
                        parent_tool_use_id,
                    } => Some(PortableBlock::ToolUse {
                        id: id.clone(),
                        name: name.clone(),
                        input_summary: input.to_string(),
                        input: input.clone(),
                        parent_tool_use_id: parent_tool_use_id.clone(),
                    }),
                    ContentBlock::ToolResult { tool_use_id, content, .. } => Some(PortableBlock::ToolResult {
                        id: tool_use_id.clone(),
                        output_summary: content.clone(),
                        output: Some(content.clone()),
                    }),
                    ContentBlock::Thinking { .. } | ContentBlock::Image { .. } => None,
                })
                .collect();
            let text = blocks
                .iter()
                .filter_map(|b| match b {
                    PortableBlock::Text { text } => Some(text.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("\n");
            PortableMessage {
                role: m.role.clone(),
                text,
                blocks,
            }
        })
        .collect()
}

impl<N, C, J, S, Notif, R> MultiRunnerAgent<N, C, J, S, Notif, R>
where
    N: RequestClient + PublishClient + SubscribeClient + FlushClient + Clone + Send + Sync + 'static,
    C: GetElapsed + Clone + Send + Sync + 'static,
    C::Instant: Send,
    J: JetStreamPublisher + JetStreamGetStream + Clone + Send + Sync + 'static,
    JsMessageOf<J>: JsRequestMessage,
    S: SessionStore + Clone + Send + Sync + 'static,
    Notif: SessionNotifier + Send + Sync + 'static,
    R: trogon_registry::RegistryStore,
{
    /// Set the model for a session, routing/migrating between the embedded agent and
    /// external runners as needed. `agent-client-protocol` 1.2.0 removed the dedicated
    /// `set_session_model` agent method (and its request/response wire types) entirely, so
    /// this is a plain inherent method rather than a trait method — mirroring
    /// `TrogonAcpAgent::set_session_model`, which callers (including
    /// `set_session_config_option`'s `"model"` intercept below) call directly.
    pub(crate) async fn set_session_model(
        &self,
        session_id: SessionId,
        raw_model: impl Into<String>,
    ) -> Result<SetSessionConfigOptionResponse> {
        let acp_sid = session_id.0.to_string();
        let model = raw_model.into();
        // session_cwd is populated in new_session. For load_session / resume_session /
        // fork_session the entry is absent until the first prompt fires the lazy re-init.
        // If set_session_model is called before the first prompt (e.g. user changes model
        // immediately after loading), fall back to the KV store so the runner session
        // gets the real working directory instead of ".".
        let existing_cwd = self
            .session_cwd
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&acp_sid)
            .cloned();
        let cwd = match existing_cwd {
            Some(c) => c,
            None => match self.store.load(&acp_sid).await {
                Ok(s) => {
                    self.session_cwd
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .insert(acp_sid.clone(), s.cwd.clone());
                    s.cwd
                }
                Err(_) => ".".to_string(),
            },
        };
        let previous_model = self
            .store
            .load(&acp_sid)
            .await
            .ok()
            .and_then(|state| state.model)
            .unwrap_or_else(|| self.inner.default_model.clone());
        // Keep the inner agent's model state in sync (drives the IDE model selector).
        let resp = self.inner.set_session_model(session_id.clone(), model.clone()).await?;

        // Where the conversation currently lives, and where it should go.
        let current = self
            .active_sessions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&acp_sid)
            .cloned();
        let (source_prefix, source_sid) = match current {
            Some((p, s)) => (p, s),
            None => (self.embedded_prefix.clone(), acp_sid.clone()),
        };
        let target_prefix = self
            .resolve_external_prefix(&model)
            .await
            .unwrap_or_else(|| self.embedded_prefix.clone());

        if target_prefix == source_prefix {
            // Same runner — no migration. Update the model on the runner session
            // (NewSessionRequest had no model field). For the embedded Claude,
            // inner.set_session_model already applied it above.
            if target_prefix != self.embedded_prefix
                && let Some(bridge) = self.get_or_create_bridge(&source_prefix)
            {
                let _ = bridge
                    .set_session_config_option(SetSessionConfigOptionRequest::new(
                        SessionId::new(source_sid.clone()),
                        "model",
                        model.as_str(),
                    ))
                    .await;
            }
            return Ok(resp);
        }

        let canonical_switcher = self.canonical_switcher.as_ref().cloned();
        let canonical_switching_enabled = match canonical_switcher.as_ref() {
            Some(switcher) => switcher.lock().await.kernel_flags().use_canonical_runner_binding(),
            None => false,
        };
        if canonical_switching_enabled {
            let canonical_switcher = canonical_switcher.expect("checked above");
            let target_existing_session = (target_prefix == self.embedded_prefix).then_some(acp_sid.as_str());
            let surface = canonical_switcher
                .lock()
                .await
                .switch_model_for_session_into(
                    &source_prefix,
                    &source_sid,
                    &acp_sid,
                    target_existing_session,
                    &previous_model,
                    &model,
                    &cwd,
                )
                .await
                .map_err(|err| Error::new(ErrorCode::InternalError.into(), err))?;

            if surface.new_prefix == self.embedded_prefix {
                self.active_sessions
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .remove(&acp_sid);
            } else {
                self.active_sessions
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .insert(acp_sid.clone(), (surface.new_prefix.clone(), surface.new_session_id.clone()));
                self.id_remap
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .insert(surface.new_session_id.clone(), acp_sid.clone());
            }
            if source_prefix != self.embedded_prefix && source_sid != surface.new_session_id {
                self.close_runner_session(&source_prefix, &source_sid).await;
                self.id_remap
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .remove(&source_sid);
            }
            return Ok(resp);
        }

        // Legacy/MVP compatibility: migrate history by exporting from the source runner
        // and importing into the target. This path is intentionally bypassed whenever the
        // Session Kernel is configured for canonical runner bindings.
        // (Lossy across providers by design: tool-calls→text, thinking/image dropped.)
        let messages = self.export_history(&source_prefix, &source_sid).await;
        if target_prefix == self.embedded_prefix {
            // Back to the embedded Claude: import into its EXISTING acp_sid session
            // (shared store with inner) so the IDE session id stays stable and Claude's
            // per-session MCP bridges remain intact. Then route prompts back to inner.
            if let Some(m) = &messages {
                self.import_history(&target_prefix, &acp_sid, m).await;
            }
            self.active_sessions
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .remove(&acp_sid);
            // Always free the old external runner when going back to Claude.
            if source_prefix != self.embedded_prefix {
                self.close_runner_session(&source_prefix, &source_sid).await;
                self.id_remap
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .remove(&source_sid);
            }
        } else if let Some(target_sid) = self.open_runner_session(&target_prefix, &cwd, &model).await {
            if let Some(m) = &messages {
                self.import_history(&target_prefix, &target_sid, m).await;
            }
            self.active_sessions
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .insert(acp_sid.clone(), (target_prefix.clone(), target_sid.clone()));
            self.id_remap
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .insert(target_sid, acp_sid.clone());
            // Free the old external runner only after the new one is open. If
            // open_runner_session returned None (target temporarily unavailable),
            // the source session stays open and routing is unchanged so the next
            // prompt still works — the user can retry the model switch later.
            if source_prefix != self.embedded_prefix {
                self.close_runner_session(&source_prefix, &source_sid).await;
                self.id_remap
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .remove(&source_sid);
            }
        }

        Ok(resp)
    }
}

#[async_trait::async_trait]
impl<N, C, J, S, Notif, R> AgentHandler for MultiRunnerAgent<N, C, J, S, Notif, R>
where
    N: RequestClient + PublishClient + SubscribeClient + FlushClient + Clone + Send + Sync + 'static,
    C: GetElapsed + Clone + Send + Sync + 'static,
    C::Instant: Send,
    J: JetStreamPublisher + JetStreamGetStream + Clone + Send + Sync + 'static,
    JsMessageOf<J>: JsRequestMessage,
    S: SessionStore + Clone + Send + Sync + 'static,
    Notif: SessionNotifier + Send + Sync + 'static,
    R: trogon_registry::RegistryStore,
{
    // ── Routed methods ───────────────────────────────────────────────────────────

    async fn new_session(&self, args: NewSessionRequest) -> Result<NewSessionResponse> {
        // Capture cwd so a runner session can be opened on model selection.
        let cwd = args.cwd.to_string_lossy().to_string();
        let resp = self.inner.new_session(args).await?;
        let acp_sid = resp.session_id.0.to_string();
        self.session_cwd
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(acp_sid.clone(), cwd.clone());

        // If the deploy-time default model (AGENT_MODEL) is served by an external runner,
        // establish routing now so the FIRST prompt reaches that runner. Without this the
        // first prompt falls through to the embedded bridge, whose JetStream streams
        // trogon-acp never provisions → "get notifications stream: stream not found".
        // Best-effort: if the runner is unavailable the session still succeeds and the
        // user can select a model later (set_session_model sets up routing then).
        let default_model = self.inner.default_model.clone();
        if let Some(prefix) = self.resolve_external_prefix(&default_model).await
            && let Some(runner_sid) = self.open_runner_session(&prefix, &cwd, &default_model).await
        {
            self.active_sessions
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .insert(acp_sid.clone(), (prefix.clone(), runner_sid.clone()));
            self.id_remap
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .insert(runner_sid, acp_sid.clone());
            // Persist the chosen model to KV so load_session / fork_session / resume_session
            // and a process restart re-route correctly via the lazy path in prompt().
            if let Ok(mut state) = self.store.load(&acp_sid).await
                && state.model.is_none()
            {
                state.model = Some(default_model);
                let _ = self.store.save(&acp_sid, &state).await;
            }
        }

        // Replace inner's Claude-only model list with the full multi-runner list.
        let current = Self::current_model_from_config_options(&resp.config_options)
            .unwrap_or_else(|| self.inner.default_model.clone());
        let config_options = self.overlay_registry_models(resp.config_options.clone(), &current).await;
        Ok(resp.config_options(config_options))
    }

    async fn prompt(&self, args: PromptRequest) -> Result<PromptResponse> {
        let acp_sid = args.session_id.0.to_string();
        // Fast path: already routed to an external runner.
        if let Some((prefix, runner_sid)) = self.route_of(&acp_sid)
            && let Some(bridge) = self.get_or_create_bridge(&prefix)
        {
            let resp = bridge.prompt_to(args, &runner_sid).await?;
            self.post_prompt_sync_kv(&acp_sid, &prefix, &runner_sid).await;
            return Ok(resp);
        }
        // One-time lazy re-init for sessions not seen in this process lifetime:
        // process restart, fork_session, load_session, or resume_session all produce
        // session ids absent from session_cwd (populated only in new_session). Load KV
        // state once, re-open the external runner session if the model requires it, and
        // register routing — subsequent prompts hit the fast path above.
        let known = self
            .session_cwd
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .contains_key(&acp_sid);
        if !known && let Ok(state) = self.store.load(&acp_sid).await {
            self.session_cwd
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .insert(acp_sid.clone(), state.cwd.clone());
            if let Some(ref model) = state.model
                && let Some(prefix) = self.resolve_external_prefix(model).await
            {
                let cwd = state.cwd.clone();
                if let Some(runner_sid) = self.open_runner_session(&prefix, &cwd, model).await {
                    if !state.messages.is_empty() {
                        self.import_history(&prefix, &runner_sid, &state.messages).await;
                    }
                    self.active_sessions
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .insert(acp_sid.clone(), (prefix.clone(), runner_sid.clone()));
                    self.id_remap
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .insert(runner_sid.clone(), acp_sid.clone());
                    if let Some(bridge) = self.get_or_create_bridge(&prefix) {
                        let resp = bridge.prompt_to(args, &runner_sid).await?;
                        self.post_prompt_sync_kv(&acp_sid, &prefix, &runner_sid).await;
                        return Ok(resp);
                    }
                }
            }
        }
        self.inner.prompt(args).await
    }

    async fn cancel(&self, args: CancelNotification) -> Result<()> {
        let acp_sid = args.session_id.0.to_string();
        if let Some((prefix, runner_sid)) = self.route_of(&acp_sid)
            && let Some(bridge) = self.get_or_create_bridge(&prefix)
        {
            return bridge.cancel_to(args, &runner_sid).await;
        }
        self.inner.cancel(args).await
    }

    // ── Delegated unchanged to the embedded agent ─────────────────────────────────

    async fn initialize(&self, args: InitializeRequest) -> Result<InitializeResponse> {
        self.inner.initialize(args).await
    }

    async fn authenticate(&self, args: AuthenticateRequest) -> Result<AuthenticateResponse> {
        self.inner.authenticate(args).await
    }

    async fn logout(&self, args: LogoutRequest) -> Result<LogoutResponse> {
        self.inner.logout(args).await
    }

    async fn load_session(&self, args: LoadSessionRequest) -> Result<LoadSessionResponse> {
        let resp = self.inner.load_session(args).await?;
        let current = Self::current_model_from_config_options(&resp.config_options)
            .unwrap_or_else(|| self.inner.default_model.clone());
        let config_options = self.overlay_registry_models(resp.config_options.clone(), &current).await;
        Ok(resp.config_options(config_options))
    }

    async fn set_session_mode(&self, args: SetSessionModeRequest) -> Result<SetSessionModeResponse> {
        self.inner.set_session_mode(args).await
    }

    async fn list_sessions(&self, args: ListSessionsRequest) -> Result<ListSessionsResponse> {
        self.inner.list_sessions(args).await
    }

    async fn set_session_config_option(
        &self,
        args: SetSessionConfigOptionRequest,
    ) -> Result<SetSessionConfigOptionResponse> {
        // Intercept the "model" key so that /model grok-3 (which calls
        // set_session_config_option("model", "grok-3")) goes through MultiRunnerAgent's
        // full routing logic instead of TrogonAcpAgent's Claude-only resolve_model.
        if args.config_id.0.as_ref() == "model" {
            let model_id = match &args.value {
                SessionConfigOptionValue::ValueId { value } => value.to_string(),
                other => format!("{other:?}"),
            };
            self.set_session_model(args.session_id.clone(), model_id).await?;
            // The ConfigOptionUpdate notification (sent inside set_session_model via
            // inner.set_session_model) updates the IDE config panel. Return empty
            // config_options — the notification is the authoritative update path.
            return Ok(SetSessionConfigOptionResponse::new(vec![]));
        }

        // Forward "compactor_model" to the active external runner so its in-memory
        // session state is updated. The shared KV (written by inner below) only reaches
        // the embedded acp-runner; xai-runner and openrouter-runner keep compactor_model
        // keyed by runner_sid. Pattern mirrors open_runner_session: try the bridge first;
        // if the runner is temporarily down, fall through to inner (best-effort).
        if args.config_id.0.as_ref() == "compactor_model" {
            let acp_sid = args.session_id.0.to_string();
            // Best-effort forward to the external runner; on any miss (no route,
            // not external, no bridge, or a forward error) fall through to inner.
            if let Some((prefix, runner_sid)) = self.route_of(&acp_sid)
                && prefix != self.embedded_prefix
                && let Some(bridge) = self.get_or_create_bridge(&prefix)
            {
                let mut ext_args = args.clone();
                ext_args.session_id = runner_sid.into();
                match bridge.set_session_config_option(ext_args).await {
                    Ok(resp) => return Ok(resp),
                    Err(e) => warn!(
                        error = %e, prefix,
                        "multi-runner: compactor_model forward failed — falling to inner"
                    ),
                }
            }
            // Bridge unavailable: fall through to inner (same best-effort
            // pattern as open_runner_session returning None → caller uses inner).
        }

        self.inner.set_session_config_option(args).await
    }

    async fn fork_session(&self, args: ForkSessionRequest) -> Result<ForkSessionResponse> {
        let resp = self.inner.fork_session(args).await?;
        let current = Self::current_model_from_config_options(&resp.config_options)
            .unwrap_or_else(|| self.inner.default_model.clone());
        let config_options = self.overlay_registry_models(resp.config_options.clone(), &current).await;
        Ok(resp.config_options(config_options))
    }

    async fn resume_session(&self, args: ResumeSessionRequest) -> Result<ResumeSessionResponse> {
        let resp = self.inner.resume_session(args).await?;
        let current = Self::current_model_from_config_options(&resp.config_options)
            .unwrap_or_else(|| self.inner.default_model.clone());
        let config_options = self.overlay_registry_models(resp.config_options.clone(), &current).await;
        Ok(resp.config_options(config_options))
    }

    async fn close_session(&self, args: CloseSessionRequest) -> Result<CloseSessionResponse> {
        let acp_sid = args.session_id.0.to_string();
        // If this session is routed to an external runner, close that runner session too
        // and drop its routing/remap/cwd entries. (Mutex lock released before .await.)
        let route = self
            .active_sessions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&acp_sid);
        if let Some((prefix, runner_sid)) = route {
            self.id_remap
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .remove(&runner_sid);
            self.close_runner_session(&prefix, &runner_sid).await;
        }
        self.session_cwd
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&acp_sid);
        self.inner.close_session(args).await
    }

    async fn ext_method(&self, args: ExtRequest) -> Result<ExtResponse> {
        self.inner.ext_method(args).await
    }

    async fn ext_notification(&self, args: ExtNotification) -> Result<()> {
        self.inner.ext_notification(args).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use acp_nats::{AcpPrefix, Bridge, Config, NatsAuth, NatsConfig};
    use tokio::sync::{RwLock, mpsc};
    use trogon_acp_runner::{
        SessionState, session_notifier::mock::MockSessionNotifier, session_store::mock::MemorySessionStore,
    };
    use trogon_nats::{
        AdvancedMockNatsClient,
        jetstream::{MockJetStreamConsumerFactory, MockJetStreamPublisher, MockJetStreamStream},
        mocks::MockError,
    };
    use trogon_registry::{AgentCapability, MockRegistryStore, Registry};
    use trogon_std::time::SystemClock;

    // ── Mock JetStream ────────────────────────────────────────────────────────

    #[derive(Clone)]
    struct MockJs {
        publisher: MockJetStreamPublisher,
        consumer_factory: MockJetStreamConsumerFactory,
    }

    impl MockJs {
        fn new() -> Self {
            Self {
                publisher: MockJetStreamPublisher::new(),
                consumer_factory: MockJetStreamConsumerFactory::new(),
            }
        }
    }

    impl trogon_nats::jetstream::JetStreamPublisher for MockJs {
        type PublishError = MockError;
        type AckFuture =
            std::future::Ready<std::result::Result<async_nats::jetstream::publish::PublishAck, Self::PublishError>>;

        async fn publish_with_headers<S: async_nats::subject::ToSubject + Send>(
            &self,
            subject: S,
            headers: async_nats::HeaderMap,
            payload: bytes::Bytes,
        ) -> std::result::Result<Self::AckFuture, Self::PublishError> {
            self.publisher.publish_with_headers(subject, headers, payload).await
        }
    }

    impl trogon_nats::jetstream::JetStreamGetStream for MockJs {
        type Error = trogon_nats::jetstream::GetStreamError;
        type Stream = MockJetStreamStream;

        async fn get_stream<T: AsRef<str> + Send>(
            &self,
            stream_name: T,
        ) -> std::result::Result<MockJetStreamStream, Self::Error> {
            self.consumer_factory.get_stream(stream_name).await
        }
    }

    // ── Test type ─────────────────────────────────────────────────────────────

    type TestAgent = MultiRunnerAgent<
        AdvancedMockNatsClient,
        SystemClock,
        MockJs,
        MemorySessionStore,
        MockSessionNotifier,
        MockRegistryStore,
    >;

    const EMBEDDED_PREFIX: &str = "acp";
    const EXT_PREFIX: &str = "xai";
    const EXT_MODEL: &str = "grok-4";
    const EXT_RUNNER_SID: &str = "runner-session-001";

    // ── Helpers ───────────────────────────────────────────────────────────────

    fn build_agent(
        nats: AdvancedMockNatsClient,
        store: MemorySessionStore,
        registry: Registry<MockRegistryStore>,
        notif_tx: mpsc::Sender<SessionNotification>,
    ) -> TestAgent {
        let js = MockJs::new();
        let notifier = MockSessionNotifier::new();
        let gateway_config = Arc::new(RwLock::new(None));
        let config = Config::new(
            AcpPrefix::new(EMBEDDED_PREFIX).unwrap(),
            NatsConfig {
                servers: vec!["unused".into()],
                auth: NatsAuth::None,
            },
        );
        let bridge = Bridge::new(
            nats.clone(),
            js.clone(),
            SystemClock,
            &opentelemetry::global::meter("multi-runner-test"),
            config.clone(),
            notif_tx.clone(),
        );
        let inner = crate::agent::TrogonAcpAgent::new(
            bridge,
            store.clone(),
            notifier,
            EMBEDDED_PREFIX,
            notif_tx.clone(),
            "claude-opus-4-6",
            gateway_config,
        );
        MultiRunnerAgent::new(
            inner,
            store,
            nats,
            js,
            SystemClock,
            config,
            registry,
            notif_tx,
            EMBEDDED_PREFIX,
        )
    }

    fn make_agent() -> (TestAgent, AdvancedMockNatsClient, MemorySessionStore) {
        let nats = AdvancedMockNatsClient::new();
        let store = MemorySessionStore::new();
        let registry = Registry::new(MockRegistryStore::new());
        let (notif_tx, _) = mpsc::channel(64);
        (
            build_agent(nats.clone(), store.clone(), registry, notif_tx),
            nats,
            store,
        )
    }

    fn make_agent_with_registry(
        registry: Registry<MockRegistryStore>,
    ) -> (TestAgent, AdvancedMockNatsClient, MemorySessionStore) {
        let nats = AdvancedMockNatsClient::new();
        let store = MemorySessionStore::new();
        let (notif_tx, _) = mpsc::channel(64);
        (
            build_agent(nats.clone(), store.clone(), registry, notif_tx),
            nats,
            store,
        )
    }

    /// Extract the "model" config option's select choices from a session response,
    /// mirroring what the old `SessionModelState::available_models` field represented.
    fn model_ids_from_config_options(config_options: &Option<Vec<SessionConfigOption>>) -> Vec<String> {
        let Some(opts) = config_options else { return vec![] };
        let Some(model_opt) = opts.iter().find(|o| o.id.0.as_ref() == "model") else {
            return vec![];
        };
        let SessionConfigKind::Select(select) = &model_opt.kind else {
            return vec![];
        };
        match &select.options {
            agent_client_protocol::schema::v1::SessionConfigSelectOptions::Ungrouped(list) => {
                list.iter().map(|o| o.value.0.to_string()).collect()
            }
            agent_client_protocol::schema::v1::SessionConfigSelectOptions::Grouped(_) => vec![],
            _ => vec![],
        }
    }

    /// Register an external model in the registry pointing to `ext_prefix`.
    async fn register_ext_model(registry: &Registry<MockRegistryStore>, model: &str, ext_prefix: &str) {
        let mut cap = AgentCapability::new(ext_prefix, ["chat"], format!("{ext_prefix}.>"));
        cap.metadata = serde_json::json!({ "models": [model], "acp_prefix": ext_prefix });
        registry.register(&cap).await.unwrap();
    }

    /// Register multiple models on one runner.
    async fn register_models_on_runner(registry: &Registry<MockRegistryStore>, models: &[&str], ext_prefix: &str) {
        let mut cap = AgentCapability::new(ext_prefix, ["chat"], format!("{ext_prefix}.>"));
        cap.metadata = serde_json::json!({ "models": models, "acp_prefix": ext_prefix });
        registry.register(&cap).await.unwrap();
    }

    /// Pre-program the mock NATS to respond successfully to `new_session` on `ext_prefix`.
    fn stub_ext_new_session(nats: &AdvancedMockNatsClient, ext_prefix: &str, runner_sid: &str) {
        let resp = NewSessionResponse::new(SessionId::from(runner_sid.to_string()));
        let bytes = serde_json::to_vec(&resp).unwrap();
        nats.set_response(&format!("{ext_prefix}.agent.session.new"), bytes.into());
    }

    async fn run_local<F, Fut>(f: F)
    where
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = ()>,
    {
        tokio::task::LocalSet::new().run_until(f()).await;
    }

    // ── new_session ───────────────────────────────────────────────────────────

    #[tokio::test(flavor = "current_thread")]
    async fn new_session_captures_cwd() {
        run_local(|| async {
            let (agent, _, _) = make_agent();
            let resp = agent
                .new_session(NewSessionRequest::new("/project/root").mcp_servers(vec![]))
                .await
                .unwrap();
            let sid = resp.session_id.0.to_string();
            let cwd = agent.session_cwd_snapshot();
            assert_eq!(
                cwd.get(&sid),
                Some(&"/project/root".to_string()),
                "session_cwd must record the cwd from new_session"
            );
        })
        .await;
    }

    #[tokio::test(flavor = "current_thread")]
    async fn new_session_injects_models_from_populated_registry() {
        run_local(|| async {
            let reg = Registry::new(MockRegistryStore::new());
            register_ext_model(&reg, EXT_MODEL, EXT_PREFIX).await;
            let (agent, _, _) = make_agent_with_registry(reg);
            let resp = agent
                .new_session(NewSessionRequest::new("/cwd").mcp_servers(vec![]))
                .await
                .unwrap();
            let ids = model_ids_from_config_options(&resp.config_options);
            let ids: Vec<&str> = ids.iter().map(String::as_str).collect();
            assert!(
                ids.contains(&EXT_MODEL),
                "registry model must appear in available_models: {ids:?}"
            );
        })
        .await;
    }

    #[tokio::test(flavor = "current_thread")]
    async fn new_session_keeps_inner_models_when_registry_empty() {
        run_local(|| async {
            let (agent, _, _) = make_agent();
            let resp = agent
                .new_session(NewSessionRequest::new("/cwd").mcp_servers(vec![]))
                .await
                .unwrap();
            let ids = model_ids_from_config_options(&resp.config_options);
            let ids: Vec<&str> = ids.iter().map(String::as_str).collect();
            // Claude model must be present; ext model must not be
            assert!(
                ids.iter().any(|id| id.contains("claude")),
                "Claude model must be in list: {ids:?}"
            );
            assert!(
                !ids.contains(&EXT_MODEL),
                "registry is empty — ext model must not appear: {ids:?}"
            );
        })
        .await;
    }

    // ── load / fork / resume — registry model injection ───────────────────────

    #[tokio::test(flavor = "current_thread")]
    async fn load_session_injects_models_from_registry() {
        run_local(|| async {
            let reg = Registry::new(MockRegistryStore::new());
            register_ext_model(&reg, EXT_MODEL, EXT_PREFIX).await;
            let (agent, _, _) = make_agent_with_registry(reg);
            let new_resp = agent
                .new_session(NewSessionRequest::new("/cwd").mcp_servers(vec![]))
                .await
                .unwrap();
            let sid = new_resp.session_id.clone();
            let load_resp = agent.load_session(LoadSessionRequest::new(sid, "/cwd")).await.unwrap();
            let ids = model_ids_from_config_options(&load_resp.config_options);
            let ids: Vec<&str> = ids.iter().map(String::as_str).collect();
            assert!(
                ids.contains(&EXT_MODEL),
                "registry model must appear after load_session: {ids:?}"
            );
        })
        .await;
    }

    #[tokio::test(flavor = "current_thread")]
    async fn fork_session_injects_models_from_registry() {
        run_local(|| async {
            let reg = Registry::new(MockRegistryStore::new());
            register_ext_model(&reg, EXT_MODEL, EXT_PREFIX).await;
            let (agent, _, _) = make_agent_with_registry(reg);
            let new_resp = agent
                .new_session(NewSessionRequest::new("/cwd").mcp_servers(vec![]))
                .await
                .unwrap();
            let sid = new_resp.session_id.clone();
            let fork_resp = agent
                .fork_session(ForkSessionRequest::new(sid, "/forked-cwd"))
                .await
                .unwrap();
            let ids = model_ids_from_config_options(&fork_resp.config_options);
            let ids: Vec<&str> = ids.iter().map(String::as_str).collect();
            assert!(
                ids.contains(&EXT_MODEL),
                "registry model must appear after fork_session: {ids:?}"
            );
        })
        .await;
    }

    #[tokio::test(flavor = "current_thread")]
    async fn resume_session_injects_models_from_registry() {
        run_local(|| async {
            let reg = Registry::new(MockRegistryStore::new());
            register_ext_model(&reg, EXT_MODEL, EXT_PREFIX).await;
            let (agent, _, _) = make_agent_with_registry(reg);
            let new_resp = agent
                .new_session(NewSessionRequest::new("/cwd").mcp_servers(vec![]))
                .await
                .unwrap();
            let sid = new_resp.session_id.clone();
            let resume_resp = agent
                .resume_session(ResumeSessionRequest::new(sid, "/cwd"))
                .await
                .unwrap();
            let ids = model_ids_from_config_options(&resume_resp.config_options);
            let ids: Vec<&str> = ids.iter().map(String::as_str).collect();
            assert!(
                ids.contains(&EXT_MODEL),
                "registry model must appear after resume_session: {ids:?}"
            );
        })
        .await;
    }

    // ── build_model_state_from_registry ───────────────────────────────────────

    #[tokio::test(flavor = "current_thread")]
    async fn build_model_state_deduplicates_models_across_runners() {
        run_local(|| async {
            let reg = Registry::new(MockRegistryStore::new());
            // "grok-4" advertised by both runners — must appear only once in the list.
            register_models_on_runner(&reg, &["grok-4", "grok-3"], EXT_PREFIX).await;
            register_models_on_runner(&reg, &["grok-4", "gemini-pro"], "openrouter").await;
            let (agent, _, _) = make_agent_with_registry(reg);
            let resp = agent
                .new_session(NewSessionRequest::new("/cwd").mcp_servers(vec![]))
                .await
                .unwrap();
            let ids = model_ids_from_config_options(&resp.config_options);
            let ids: Vec<&str> = ids.iter().map(String::as_str).collect();
            assert_eq!(
                ids.iter().filter(|&&id| id == "grok-4").count(),
                1,
                "grok-4 must appear exactly once even when two runners advertise it: {ids:?}"
            );
            assert!(ids.contains(&"grok-3"), "grok-3 must be present: {ids:?}");
            assert!(ids.contains(&"gemini-pro"), "gemini-pro must be present: {ids:?}");
        })
        .await;
    }

    #[tokio::test(flavor = "current_thread")]
    async fn build_model_state_returns_none_for_empty_registry() {
        run_local(|| async {
            // Empty registry → overlay_registry_models returns config_options unmodified →
            // inner agent's model list is returned unmodified.
            let (agent, _, _) = make_agent();
            let resp = agent
                .new_session(NewSessionRequest::new("/cwd").mcp_servers(vec![]))
                .await
                .unwrap();
            let ids = model_ids_from_config_options(&resp.config_options);
            // The list comes from the embedded agent, not the registry; ext model absent.
            assert!(
                !ids.iter().any(|id| id == EXT_MODEL),
                "ext model must not appear when registry is empty"
            );
        })
        .await;
    }

    // ── set_session_model — routing table management ──────────────────────────

    #[tokio::test(flavor = "current_thread")]
    async fn set_session_model_claude_stays_on_embedded() {
        run_local(|| async {
            let (agent, _, _) = make_agent();
            let resp = agent
                .new_session(NewSessionRequest::new("/cwd").mcp_servers(vec![]))
                .await
                .unwrap();
            let sid = resp.session_id.0.to_string();
            agent
                .set_session_model(SessionId::new(sid.clone()), "claude-opus-4-6")
                .await
                .unwrap();
            // No external routing should be created for a Claude model.
            assert!(
                agent.active_sessions_snapshot().is_empty(),
                "Claude model must not create an external routing entry"
            );
            assert!(
                agent.id_remap_handle().lock().unwrap_or_else(std::sync::PoisonError::into_inner).is_empty(),
                "id_remap must be empty when no external runner is used"
            );
        })
        .await;
    }

    #[tokio::test(flavor = "current_thread")]
    async fn set_session_model_external_fails_gracefully_no_state_corruption() {
        run_local(|| async {
            let reg = Registry::new(MockRegistryStore::new());
            register_ext_model(&reg, EXT_MODEL, EXT_PREFIX).await;
            let (agent, _, _) = make_agent_with_registry(reg);
            let resp = agent
                .new_session(NewSessionRequest::new("/cwd").mcp_servers(vec![]))
                .await
                .unwrap();
            let sid = resp.session_id.0.to_string();
            // No mock response programmed → open_runner_session returns None → no routing.
            agent
                .set_session_model(SessionId::new(sid.clone()), EXT_MODEL)
                .await
                .unwrap();
            assert!(
                agent.active_sessions_snapshot().is_empty(),
                "failed open_runner_session must not create a routing entry"
            );
            assert!(
                agent.id_remap_handle().lock().unwrap_or_else(std::sync::PoisonError::into_inner).is_empty(),
                "id_remap must be empty when open_runner_session fails"
            );
        })
        .await;
    }

    #[tokio::test(flavor = "current_thread")]
    async fn set_session_model_external_succeeds_inserts_routing() {
        run_local(|| async {
            let reg = Registry::new(MockRegistryStore::new());
            register_ext_model(&reg, EXT_MODEL, EXT_PREFIX).await;
            let (agent, nats, _) = make_agent_with_registry(reg);
            let resp = agent
                .new_session(NewSessionRequest::new("/cwd").mcp_servers(vec![]))
                .await
                .unwrap();
            let sid = resp.session_id.0.to_string();
            // Pre-program the mock so open_runner_session succeeds.
            stub_ext_new_session(&nats, EXT_PREFIX, EXT_RUNNER_SID);
            agent
                .set_session_model(SessionId::new(sid.clone()), EXT_MODEL)
                .await
                .unwrap();
            // Routing must be inserted.
            let sessions = agent.active_sessions_snapshot();
            assert_eq!(
                sessions.get(&sid),
                Some(&(EXT_PREFIX.to_string(), EXT_RUNNER_SID.to_string())),
                "active_sessions must record (prefix, runner_sid) after successful migration"
            );
            // id_remap must contain the reverse mapping.
            let remap = agent.id_remap_handle();
            assert_eq!(
                remap.lock().unwrap_or_else(std::sync::PoisonError::into_inner).get(EXT_RUNNER_SID).cloned(),
                Some(sid.clone()),
                "id_remap must map runner_sid → acp_sid"
            );
        })
        .await;
    }

    #[tokio::test(flavor = "current_thread")]
    async fn set_session_model_back_to_claude_removes_routing() {
        run_local(|| async {
            let reg = Registry::new(MockRegistryStore::new());
            register_ext_model(&reg, EXT_MODEL, EXT_PREFIX).await;
            let (agent, nats, _) = make_agent_with_registry(reg);
            let resp = agent
                .new_session(NewSessionRequest::new("/cwd").mcp_servers(vec![]))
                .await
                .unwrap();
            let sid = resp.session_id.0.to_string();
            // Establish external routing.
            stub_ext_new_session(&nats, EXT_PREFIX, EXT_RUNNER_SID);
            agent
                .set_session_model(SessionId::new(sid.clone()), EXT_MODEL)
                .await
                .unwrap();
            assert!(
                !agent.active_sessions_snapshot().is_empty(),
                "routing must be established first"
            );
            // Switch back to Claude.
            agent
                .set_session_model(SessionId::new(sid.clone()), "claude-opus-4-6")
                .await
                .unwrap();
            assert!(
                agent.active_sessions_snapshot().is_empty(),
                "switching back to Claude must remove the external routing entry"
            );
            assert!(
                agent.id_remap_handle().lock().unwrap_or_else(std::sync::PoisonError::into_inner).is_empty(),
                "id_remap must be cleared after switching back to Claude"
            );
        })
        .await;
    }

    /// Bug fix 1: if `open_runner_session` for target B fails, source A routing stays intact.
    #[tokio::test(flavor = "current_thread")]
    async fn set_session_model_migration_failure_keeps_source_routing() {
        run_local(|| async {
            let reg = Registry::new(MockRegistryStore::new());
            register_ext_model(&reg, EXT_MODEL, EXT_PREFIX).await;
            register_ext_model(&reg, "gemini-pro", "openrouter").await;
            let (agent, nats, _) = make_agent_with_registry(reg);
            let resp = agent
                .new_session(NewSessionRequest::new("/cwd").mcp_servers(vec![]))
                .await
                .unwrap();
            let sid = resp.session_id.0.to_string();
            // Establish routing on "xai".
            stub_ext_new_session(&nats, EXT_PREFIX, EXT_RUNNER_SID);
            agent
                .set_session_model(SessionId::new(sid.clone()), EXT_MODEL)
                .await
                .unwrap();
            assert!(
                agent.id_remap_handle().lock().unwrap_or_else(std::sync::PoisonError::into_inner).contains_key(EXT_RUNNER_SID),
                "xai routing must be established"
            );
            // Try to migrate to "openrouter" — no mock response → open_runner_session fails.
            agent
                .set_session_model(SessionId::new(sid.clone()), "gemini-pro")
                .await
                .unwrap();
            // Source xai routing must still be intact (bug fix: old runner stays open when
            // new one fails to open).
            let sessions = agent.active_sessions_snapshot();
            assert_eq!(
                sessions.get(&sid).map(|(p, _)| p.as_str()),
                Some(EXT_PREFIX),
                "source prefix must be unchanged when target runner fails to open: {sessions:?}"
            );
            assert!(
                agent.id_remap_handle().lock().unwrap_or_else(std::sync::PoisonError::into_inner).contains_key(EXT_RUNNER_SID),
                "source runner_sid must remain in id_remap when migration fails"
            );
        })
        .await;
    }

    /// Bug fix 2: cwd is loaded from store when session_cwd is absent (e.g. load_session
    /// followed by set_session_model before the first prompt fires lazy reinit).
    #[tokio::test(flavor = "current_thread")]
    async fn set_session_model_cwd_from_store_when_session_cwd_absent() {
        run_local(|| async {
            let reg = Registry::new(MockRegistryStore::new());
            register_ext_model(&reg, EXT_MODEL, EXT_PREFIX).await;
            // Pre-populate the store directly — simulates a session that was created in a
            // previous process lifetime (load_session scenario).
            let store = MemorySessionStore::new();
            let acp_sid = "pre-existing-session-id";
            let state = SessionState {
                cwd: "/from/store".to_string(),
                ..Default::default()
            };
            store.save(acp_sid, &state).await.unwrap();
            let nats = AdvancedMockNatsClient::new();
            let (notif_tx, _) = mpsc::channel(64);
            let agent = build_agent(nats.clone(), store, reg, notif_tx);
            // session_cwd is empty — this session was not created via new_session.
            assert!(agent.session_cwd_snapshot().is_empty());
            // Pre-program external runner to accept new_session.
            stub_ext_new_session(&nats, EXT_PREFIX, EXT_RUNNER_SID);
            agent
                .set_session_model(SessionId::new(acp_sid), EXT_MODEL)
                .await
                .unwrap();
            // session_cwd must now be populated from the store (bug fix 2).
            let cwd = agent.session_cwd_snapshot();
            assert_eq!(
                cwd.get(acp_sid),
                Some(&"/from/store".to_string()),
                "set_session_model must populate session_cwd from the KV store"
            );
        })
        .await;
    }

    // ── close_session ─────────────────────────────────────────────────────────

    #[tokio::test(flavor = "current_thread")]
    async fn close_session_clears_cwd_and_routing_tables() {
        run_local(|| async {
            let reg = Registry::new(MockRegistryStore::new());
            register_ext_model(&reg, EXT_MODEL, EXT_PREFIX).await;
            let (agent, nats, _) = make_agent_with_registry(reg);
            let resp = agent
                .new_session(NewSessionRequest::new("/cwd").mcp_servers(vec![]))
                .await
                .unwrap();
            let sid = resp.session_id.clone();
            let sid_str = sid.0.to_string();
            // Establish external routing.
            stub_ext_new_session(&nats, EXT_PREFIX, EXT_RUNNER_SID);
            agent
                .set_session_model(SessionId::new(sid_str.clone()), EXT_MODEL)
                .await
                .unwrap();
            assert!(!agent.session_cwd_snapshot().is_empty(), "cwd must be captured");
            assert!(!agent.active_sessions_snapshot().is_empty(), "routing must be set");
            assert!(!agent.id_remap_handle().lock().unwrap_or_else(std::sync::PoisonError::into_inner).is_empty(), "id_remap must be set");
            // Close the session.
            agent.close_session(CloseSessionRequest::new(sid)).await.unwrap();
            assert!(
                agent.session_cwd_snapshot().is_empty(),
                "session_cwd must be cleared after close_session"
            );
            assert!(
                agent.active_sessions_snapshot().is_empty(),
                "active_sessions must be cleared after close_session"
            );
            assert!(
                agent.id_remap_handle().lock().unwrap_or_else(std::sync::PoisonError::into_inner).is_empty(),
                "id_remap must be cleared after close_session"
            );
        })
        .await;
    }

    // ── set_session_config_option ─────────────────────────────────────────────

    #[tokio::test(flavor = "current_thread")]
    async fn set_session_config_option_model_is_intercepted() {
        run_local(|| async {
            let (agent, _, _) = make_agent();
            let resp = agent
                .new_session(NewSessionRequest::new("/cwd").mcp_servers(vec![]))
                .await
                .unwrap();
            let sid = resp.session_id.clone();
            let req = SetSessionConfigOptionRequest::new(
                sid,
                "model",
                SessionConfigOptionValue::ValueId {
                    value: "claude-sonnet-4-6".into(),
                },
            );
            let cfg_resp = agent.set_session_config_option(req).await.unwrap();
            // The intercept path returns empty config_options; the notification
            // is the authoritative update path.
            assert!(
                cfg_resp.config_options.is_empty(),
                "intercepted 'model' key must return empty config_options"
            );
        })
        .await;
    }

    #[tokio::test(flavor = "current_thread")]
    async fn set_session_config_option_other_key_passes_through_to_inner() {
        run_local(|| async {
            let (agent, _, _) = make_agent();
            let resp = agent
                .new_session(NewSessionRequest::new("/cwd").mcp_servers(vec![]))
                .await
                .unwrap();
            let sid = resp.session_id.clone();
            let req = SetSessionConfigOptionRequest::new(
                sid,
                "mode",
                SessionConfigOptionValue::ValueId { value: "plan".into() },
            );
            let cfg_resp = agent.set_session_config_option(req).await.unwrap();
            // Inner agent handles "mode" and returns a non-empty config_options list.
            assert!(
                !cfg_resp.config_options.is_empty(),
                "non-model config key must pass through to inner and return config_options"
            );
        })
        .await;
    }

    // ── cancel ────────────────────────────────────────────────────────────────

    #[tokio::test(flavor = "current_thread")]
    async fn cancel_returns_ok_when_no_external_routing() {
        run_local(|| async {
            let (agent, _, _) = make_agent();
            let resp = agent
                .new_session(NewSessionRequest::new("/cwd").mcp_servers(vec![]))
                .await
                .unwrap();
            let sid = resp.session_id.0.to_string();
            // cancel is fire-and-forget on the bridge; always returns Ok.
            let result = agent.cancel(CancelNotification::new(sid)).await;
            assert!(
                result.is_ok(),
                "cancel must return Ok when session has no external routing"
            );
        })
        .await;
    }

    // ── prompt lazy reinit ────────────────────────────────────────────────────

    #[tokio::test(flavor = "current_thread")]
    async fn prompt_lazy_reinit_populates_session_cwd_from_store() {
        run_local(|| async {
            // Simulate a session created in a previous process: present in store but not in
            // session_cwd (which is only populated via new_session or set_session_model).
            let store = MemorySessionStore::new();
            let acp_sid = "existing-session-000";
            let state = SessionState {
                cwd: "/restored/cwd".to_string(),
                ..Default::default()
            };
            store.save(acp_sid, &state).await.unwrap();
            let nats = AdvancedMockNatsClient::new();
            let reg = Registry::new(MockRegistryStore::new());
            let (notif_tx, _) = mpsc::channel(64);
            let agent = build_agent(nats, store, reg, notif_tx);
            assert!(agent.session_cwd_snapshot().is_empty(), "must start with empty cwd map");
            // prompt fails (no JetStream consumers configured), but lazy reinit still runs.
            let _ = agent.prompt(PromptRequest::new(acp_sid, vec![])).await;
            assert_eq!(
                agent.session_cwd_snapshot().get(acp_sid),
                Some(&"/restored/cwd".to_string()),
                "lazy reinit must populate session_cwd from the store"
            );
        })
        .await;
    }

    #[tokio::test(flavor = "current_thread")]
    async fn prompt_returns_error_from_inner_when_no_routing() {
        run_local(|| async {
            let (agent, _, _) = make_agent();
            let resp = agent
                .new_session(NewSessionRequest::new("/cwd").mcp_servers(vec![]))
                .await
                .unwrap();
            let sid = resp.session_id.clone();
            // No external routing → falls through to inner → JetStream call fails.
            let result = agent.prompt(PromptRequest::new(sid, vec![])).await;
            assert!(
                result.is_err(),
                "prompt must propagate the inner agent error when no JetStream consumers are configured"
            );
        })
        .await;
    }

    // ── Integration tests — real NATS + JetStream + KV ───────────────────────
    //
    // Each test spins up its own NATS container (requires Docker).
    // Run with: cargo test -p trogon-acp integration
    mod integration {
        use super::super::MultiRunnerAgent;
        use acp_nats::{AcpPrefix, AgentHandler, Bridge, Config, NatsAuth, NatsConfig};
        use agent_client_protocol::schema::v1::{
            CancelNotification, CloseSessionRequest, CloseSessionResponse, ExtResponse, ForkSessionRequest,
            LoadSessionRequest, NewSessionRequest, NewSessionResponse, PromptRequest, PromptResponse,
            ResumeSessionRequest, SessionConfigOptionValue, SessionId, SetSessionConfigOptionRequest,
            SetSessionConfigOptionResponse, StopReason,
        };
        use async_nats::jetstream;
        use futures_util::StreamExt as _;
        use std::sync::Arc;
        use std::time::Duration;
        use testcontainers_modules::nats::Nats;
        use testcontainers_modules::testcontainers::{ContainerAsync, ImageExt, runners::AsyncRunner};
        use tokio::sync::mpsc;
        use trogon_acp_runner::{NatsSessionNotifier, NatsSessionStore, SessionStore};
        use trogon_nats::jetstream::NatsJetStreamClient;
        use trogon_registry::AgentCapability;
        use trogon_std::time::SystemClock;

        type RealAgent = MultiRunnerAgent<
            async_nats::Client,
            SystemClock,
            NatsJetStreamClient,
            NatsSessionStore,
            NatsSessionNotifier,
            async_nats::jetstream::kv::Store,
        >;

        const EMBEDDED: &str = "acp";
        const EXT: &str = "xai";
        const EXT_MODEL: &str = "grok-4";
        const ALT_EXT: &str = "openrouter";
        const ALT_MODEL: &str = "gemini-pro";
        const RUNNER_SID: &str = "xai-runner-001";
        const ALT_RUNNER_SID: &str = "or-runner-001";
        const SHORT_TIMEOUT: Duration = Duration::from_millis(400);

        // ── Infrastructure helpers ─────────────────────────────────────────────

        async fn start_nats() -> (ContainerAsync<Nats>, async_nats::Client, jetstream::Context) {
            let container = Nats::default()
                .with_cmd(["-js"])
                .start()
                .await
                .expect("NATS container requires Docker");
            let port = container.get_host_port_ipv4(4222).await.unwrap();
            let nats = async_nats::connect(format!("127.0.0.1:{port}")).await.unwrap();
            let js = jetstream::new(nats.clone());
            (container, nats, js)
        }

        async fn setup_streams(js: &jetstream::Context, prefix: &str) {
            let acp_prefix = AcpPrefix::new(prefix).unwrap();
            for config in acp_nats::jetstream::streams::all_configs(&acp_prefix) {
                js.get_or_create_stream(config).await.unwrap();
            }
        }

        async fn make_real_agent(
            nats: async_nats::Client,
            js: jetstream::Context,
            store: NatsSessionStore,
            registry: trogon_registry::Registry<async_nats::jetstream::kv::Store>,
        ) -> RealAgent {
            let js_client = NatsJetStreamClient::new(js);
            let (notif_tx, _notif_rx) = mpsc::channel(64);
            let config = Config::new(
                AcpPrefix::new(EMBEDDED).unwrap(),
                NatsConfig {
                    servers: vec!["unused".into()],
                    auth: NatsAuth::None,
                },
            )
            .with_operation_timeout(SHORT_TIMEOUT);
            let notifier = NatsSessionNotifier::new(nats.clone());
            let gateway_config = Arc::new(tokio::sync::RwLock::new(None));
            let bridge = Bridge::new(
                nats.clone(),
                js_client.clone(),
                SystemClock,
                &opentelemetry::global::meter("multi-runner-integration-test"),
                config.clone(),
                notif_tx.clone(),
            );
            let inner = crate::agent::TrogonAcpAgent::new(
                bridge,
                store.clone(),
                notifier,
                EMBEDDED,
                notif_tx.clone(),
                "claude-opus-4-6",
                gateway_config,
            );
            MultiRunnerAgent::new(
                inner,
                store,
                nats,
                js_client,
                SystemClock,
                config,
                registry,
                notif_tx,
                EMBEDDED,
            )
        }

        async fn register_model(
            registry: &trogon_registry::Registry<async_nats::jetstream::kv::Store>,
            model: &str,
            prefix: &str,
        ) {
            let mut cap = AgentCapability::new(prefix, ["chat"], format!("{prefix}.>"));
            cap.metadata = serde_json::json!({ "models": [model], "acp_prefix": prefix });
            registry.register(&cap).await.unwrap();
        }

        /// Spawn a background task that answers one plain-NATS new_session on
        /// `{prefix}.agent.session.new` by returning a NewSessionResponse with `runner_sid`.
        fn stub_new_session(nats: async_nats::Client, prefix: &str, runner_sid: &str) {
            let subject = format!("{prefix}.agent.session.new");
            let runner_sid = runner_sid.to_string();
            let nats2 = nats.clone();
            tokio::spawn(async move {
                let mut sub = nats2.subscribe(subject).await.unwrap();
                if let Some(msg) = sub.next().await {
                    let resp = NewSessionResponse::new(SessionId::from(runner_sid));
                    let payload = serde_json::to_vec(&resp).unwrap();
                    if let Some(reply) = msg.reply {
                        let _ = nats2.publish(reply, payload.into()).await;
                    }
                }
            });
        }

        /// Spawn a background task that answers one ext_method request on
        /// `{prefix}.agent.ext.{method}` and returns `resp_json` as the ExtResponse body.
        fn stub_ext_method(nats: async_nats::Client, prefix: &str, method: &str, resp_json: String) {
            let subject = format!("{prefix}.agent.ext.{method}");
            let nats2 = nats.clone();
            tokio::spawn(async move {
                let mut sub = nats2.subscribe(subject).await.unwrap();
                if let Some(msg) = sub.next().await {
                    let raw = serde_json::value::RawValue::from_string(resp_json).unwrap();
                    let payload = serde_json::to_vec(&ExtResponse::new(raw.into())).unwrap();
                    if let Some(reply) = msg.reply {
                        let _ = nats2.publish(reply, payload.into()).await;
                    }
                }
            });
        }

        /// Spawn a background task that answers one JetStream prompt and publishes a response.
        fn stub_js_prompt(nats: async_nats::Client, js: jetstream::Context, prefix: &str, runner_sid: &str) {
            let subject = format!("{prefix}.session.{runner_sid}.agent.prompt");
            let resp_subject_base = format!("{prefix}.session.{runner_sid}.agent.prompt.response");
            tokio::spawn(async move {
                let mut sub = nats.subscribe(subject).await.unwrap();
                if let Some(msg) = sub.next().await {
                    let req_id = msg
                        .headers
                        .as_ref()
                        .and_then(|h| h.get("X-Req-Id"))
                        .map(|v| v.as_str().to_string())
                        .unwrap_or_default();
                    let resp = serde_json::to_vec(&PromptResponse::new(StopReason::EndTurn)).unwrap();
                    let _ = js.publish(format!("{resp_subject_base}.{req_id}"), resp.into()).await;
                }
            });
        }

        /// Spawn a background task that answers one JetStream close and publishes a response.
        fn stub_js_close(nats: async_nats::Client, js: jetstream::Context, prefix: &str, runner_sid: &str) {
            let subject = format!("{prefix}.session.{runner_sid}.agent.close");
            let resp_subject_base = format!("{prefix}.session.{runner_sid}.agent.response");
            tokio::spawn(async move {
                let mut sub = nats.subscribe(subject).await.unwrap();
                if let Some(msg) = sub.next().await {
                    let req_id = msg
                        .headers
                        .as_ref()
                        .and_then(|h| h.get("X-Req-Id"))
                        .map(|v| v.as_str().to_string())
                        .unwrap_or_default();
                    let resp = serde_json::to_vec(&CloseSessionResponse::new()).unwrap();
                    let _ = js.publish(format!("{resp_subject_base}.{req_id}"), resp.into()).await;
                }
            });
        }

        /// Spawn a background task that answers one JetStream set_config_option("model", ...)
        /// and publishes a response. `agent-client-protocol` 1.2.0 removed the dedicated
        /// `set_session_model` wire method; model selection is now routed through
        /// `set_session_config_option`, so the NATS subject moved from `.agent.set_model` to
        /// `.agent.set_config_option`.
        fn stub_js_set_config_option(nats: async_nats::Client, js: jetstream::Context, prefix: &str, runner_sid: &str) {
            let subject = format!("{prefix}.session.{runner_sid}.agent.set_config_option");
            let resp_subject_base = format!("{prefix}.session.{runner_sid}.agent.response");
            tokio::spawn(async move {
                let mut sub = nats.subscribe(subject).await.unwrap();
                if let Some(msg) = sub.next().await {
                    let req_id = msg
                        .headers
                        .as_ref()
                        .and_then(|h| h.get("X-Req-Id"))
                        .map(|v| v.as_str().to_string())
                        .unwrap_or_default();
                    let resp = serde_json::to_vec(&SetSessionConfigOptionResponse::new(vec![])).unwrap();
                    let _ = js.publish(format!("{resp_subject_base}.{req_id}"), resp.into()).await;
                }
            });
        }

        // ── Tests ─────────────────────────────────────────────────────────────

        #[tokio::test(flavor = "current_thread")]
        async fn new_session_injects_models_from_real_kv_registry() {
            let (_container, nats, js) = start_nats().await;
            let store = NatsSessionStore::open(&js).await.unwrap();
            let reg_kv = trogon_registry::provision(&js).await.unwrap();
            let registry = trogon_registry::Registry::new(reg_kv);
            register_model(&registry, EXT_MODEL, EXT).await;

            let agent = make_real_agent(nats, js, store, registry).await;
            let resp = agent.new_session(NewSessionRequest::new("/cwd")).await.unwrap();

            let ids = super::model_ids_from_config_options(&resp.config_options);
            let ids: Vec<&str> = ids.iter().map(String::as_str).collect();
            assert!(
                ids.contains(&EXT_MODEL),
                "registry model must appear in new_session available_models: {ids:?}"
            );
        }

        #[tokio::test(flavor = "current_thread")]
        async fn set_session_model_records_routing_in_active_sessions() {
            let (_container, nats, js) = start_nats().await;
            setup_streams(&js, EXT).await;
            let store = NatsSessionStore::open(&js).await.unwrap();
            let reg_kv = trogon_registry::provision(&js).await.unwrap();
            let registry = trogon_registry::Registry::new(reg_kv);
            register_model(&registry, EXT_MODEL, EXT).await;

            stub_new_session(nats.clone(), EXT, RUNNER_SID);
            stub_js_set_config_option(nats.clone(), js.clone(), EXT, RUNNER_SID);

            let agent = make_real_agent(nats, js, store, registry).await;
            let new_resp = agent.new_session(NewSessionRequest::new("/cwd")).await.unwrap();
            let acp_sid = new_resp.session_id.0.to_string();

            agent
                .set_session_model(SessionId::new(acp_sid.clone()), EXT_MODEL.to_string())
                .await
                .unwrap();

            let routes = agent.active_sessions_snapshot();
            let (prefix, runner_sid) = routes
                .get(&acp_sid)
                .expect("active_sessions must contain acp_sid after routing to ext runner");
            assert_eq!(prefix, EXT, "routing prefix must be the ext runner prefix");
            assert_eq!(runner_sid, RUNNER_SID, "runner_sid must match what ext runner returned");
        }

        #[tokio::test(flavor = "current_thread")]
        async fn prompt_routes_to_external_runner_via_real_jetstream() {
            let (_container, nats, js) = start_nats().await;
            setup_streams(&js, EXT).await;
            let store = NatsSessionStore::open(&js).await.unwrap();
            let reg_kv = trogon_registry::provision(&js).await.unwrap();
            let registry = trogon_registry::Registry::new(reg_kv);
            register_model(&registry, EXT_MODEL, EXT).await;

            stub_new_session(nats.clone(), EXT, RUNNER_SID);
            stub_js_set_config_option(nats.clone(), js.clone(), EXT, RUNNER_SID);

            let agent = make_real_agent(nats.clone(), js.clone(), store, registry).await;
            let new_resp = agent.new_session(NewSessionRequest::new("/cwd")).await.unwrap();
            let acp_sid = new_resp.session_id.0.to_string();

            agent
                .set_session_model(SessionId::new(acp_sid.clone()), EXT_MODEL.to_string())
                .await
                .unwrap();

            // post_prompt_sync_kv calls session/export after a successful prompt.
            stub_js_prompt(nats.clone(), js.clone(), EXT, RUNNER_SID);
            stub_ext_method(nats.clone(), EXT, "session/export", "[]".to_string());

            let resp = agent.prompt(PromptRequest::new(acp_sid.clone(), vec![])).await;

            assert!(
                resp.is_ok(),
                "prompt must succeed when routed to external runner with real JetStream: {resp:?}"
            );
            assert_eq!(
                resp.unwrap().stop_reason,
                StopReason::EndTurn,
                "ext runner stop_reason must propagate through the routing layer"
            );
        }

        #[tokio::test(flavor = "current_thread")]
        async fn cancel_published_to_ext_runner_nats_subject() {
            let (_container, nats, js) = start_nats().await;
            setup_streams(&js, EXT).await;
            let store = NatsSessionStore::open(&js).await.unwrap();
            let reg_kv = trogon_registry::provision(&js).await.unwrap();
            let registry = trogon_registry::Registry::new(reg_kv);
            register_model(&registry, EXT_MODEL, EXT).await;

            stub_new_session(nats.clone(), EXT, RUNNER_SID);
            stub_js_set_config_option(nats.clone(), js.clone(), EXT, RUNNER_SID);

            let agent = make_real_agent(nats.clone(), js.clone(), store, registry).await;
            let new_resp = agent.new_session(NewSessionRequest::new("/cwd")).await.unwrap();
            let acp_sid = new_resp.session_id.0.to_string();

            agent
                .set_session_model(SessionId::new(acp_sid.clone()), EXT_MODEL.to_string())
                .await
                .unwrap();

            let cancel_subject = format!("{EXT}.session.{RUNNER_SID}.agent.cancel");
            let mut cancel_sub = nats.subscribe(cancel_subject.clone()).await.unwrap();

            let _ = agent.cancel(CancelNotification::new(acp_sid.clone())).await;

            let received = tokio::time::timeout(Duration::from_secs(2), cancel_sub.next()).await;
            assert!(
                received.is_ok() && received.unwrap().is_some(),
                "cancel must publish to ext runner's cancel subject: {cancel_subject}"
            );
        }

        #[tokio::test(flavor = "current_thread")]
        async fn close_session_sends_jetstream_close_to_ext_runner() {
            let (_container, nats, js) = start_nats().await;
            setup_streams(&js, EXT).await;
            let store = NatsSessionStore::open(&js).await.unwrap();
            let reg_kv = trogon_registry::provision(&js).await.unwrap();
            let registry = trogon_registry::Registry::new(reg_kv);
            register_model(&registry, EXT_MODEL, EXT).await;

            stub_new_session(nats.clone(), EXT, RUNNER_SID);
            stub_js_set_config_option(nats.clone(), js.clone(), EXT, RUNNER_SID);

            let agent = make_real_agent(nats.clone(), js.clone(), store, registry).await;
            let new_resp = agent.new_session(NewSessionRequest::new("/cwd")).await.unwrap();
            let acp_sid = new_resp.session_id.0.to_string();

            agent
                .set_session_model(SessionId::new(acp_sid.clone()), EXT_MODEL.to_string())
                .await
                .unwrap();

            let close_subject = format!("{EXT}.session.{RUNNER_SID}.agent.close");
            let mut close_sub = nats.subscribe(close_subject.clone()).await.unwrap();
            stub_js_close(nats.clone(), js.clone(), EXT, RUNNER_SID);

            agent
                .close_session(CloseSessionRequest::new(SessionId::from(acp_sid.clone())))
                .await
                .unwrap();

            let received = tokio::time::timeout(Duration::from_secs(2), close_sub.next()).await;
            assert!(
                received.is_ok() && received.unwrap().is_some(),
                "close_session must send JetStream close to ext runner: {close_subject}"
            );
        }

        #[tokio::test(flavor = "current_thread")]
        async fn close_session_removes_routing_and_deletes_from_kv_store() {
            let (_container, nats, js) = start_nats().await;
            setup_streams(&js, EXT).await;
            let store = NatsSessionStore::open(&js).await.unwrap();
            let reg_kv = trogon_registry::provision(&js).await.unwrap();
            let registry = trogon_registry::Registry::new(reg_kv);
            register_model(&registry, EXT_MODEL, EXT).await;

            stub_new_session(nats.clone(), EXT, RUNNER_SID);
            stub_js_set_config_option(nats.clone(), js.clone(), EXT, RUNNER_SID);
            stub_js_close(nats.clone(), js.clone(), EXT, RUNNER_SID);

            let store_clone = store.clone();
            let agent = make_real_agent(nats.clone(), js.clone(), store, registry).await;
            let new_resp = agent.new_session(NewSessionRequest::new("/cwd")).await.unwrap();
            let acp_sid = new_resp.session_id.0.to_string();

            agent
                .set_session_model(SessionId::new(acp_sid.clone()), EXT_MODEL.to_string())
                .await
                .unwrap();

            assert!(
                agent.active_sessions_snapshot().contains_key(&acp_sid),
                "routing must exist before close"
            );
            let state_before = store_clone.load(&acp_sid).await.unwrap();
            assert_ne!(state_before.cwd, "", "session must be in KV store before close");

            agent
                .close_session(CloseSessionRequest::new(SessionId::from(acp_sid.clone())))
                .await
                .unwrap();

            assert!(
                agent.active_sessions_snapshot().is_empty(),
                "active_sessions must be empty after close_session"
            );
            // After delete, NatsSessionStore::load returns SessionState::default() (cwd = "").
            let state_after = store_clone.load(&acp_sid).await.unwrap();
            assert_eq!(
                state_after.cwd, "",
                "KV entry must be deleted on close_session — load returns default state"
            );
        }

        #[tokio::test(flavor = "current_thread")]
        async fn load_session_injects_models_from_real_registry() {
            let (_container, nats, js) = start_nats().await;
            let store = NatsSessionStore::open(&js).await.unwrap();
            let reg_kv = trogon_registry::provision(&js).await.unwrap();
            let registry = trogon_registry::Registry::new(reg_kv);
            register_model(&registry, EXT_MODEL, EXT).await;

            let agent = make_real_agent(nats, js, store, registry).await;
            let new_resp = agent.new_session(NewSessionRequest::new("/cwd")).await.unwrap();
            let sid = new_resp.session_id.clone();

            let load_resp = agent.load_session(LoadSessionRequest::new(sid, "/cwd")).await.unwrap();

            let ids = super::model_ids_from_config_options(&load_resp.config_options);
            let ids: Vec<&str> = ids.iter().map(String::as_str).collect();
            assert!(
                ids.contains(&EXT_MODEL),
                "registry model must appear in load_session available_models: {ids:?}"
            );
        }

        #[tokio::test(flavor = "current_thread")]
        async fn fork_session_injects_models_from_real_registry() {
            let (_container, nats, js) = start_nats().await;
            let store = NatsSessionStore::open(&js).await.unwrap();
            let reg_kv = trogon_registry::provision(&js).await.unwrap();
            let registry = trogon_registry::Registry::new(reg_kv);
            register_model(&registry, EXT_MODEL, EXT).await;

            let agent = make_real_agent(nats, js, store, registry).await;
            let new_resp = agent.new_session(NewSessionRequest::new("/cwd")).await.unwrap();
            let sid = new_resp.session_id.clone();

            let fork_resp = agent
                .fork_session(ForkSessionRequest::new(sid, "/forked"))
                .await
                .unwrap();

            let ids = super::model_ids_from_config_options(&fork_resp.config_options);
            let ids: Vec<&str> = ids.iter().map(String::as_str).collect();
            assert!(
                ids.contains(&EXT_MODEL),
                "registry model must appear in fork_session available_models: {ids:?}"
            );
        }

        #[tokio::test(flavor = "current_thread")]
        async fn resume_session_injects_models_from_real_registry() {
            let (_container, nats, js) = start_nats().await;
            let store = NatsSessionStore::open(&js).await.unwrap();
            let reg_kv = trogon_registry::provision(&js).await.unwrap();
            let registry = trogon_registry::Registry::new(reg_kv);
            register_model(&registry, EXT_MODEL, EXT).await;

            let agent = make_real_agent(nats, js, store, registry).await;
            let new_resp = agent.new_session(NewSessionRequest::new("/cwd")).await.unwrap();
            let sid = new_resp.session_id.clone();

            let resume_resp = agent
                .resume_session(ResumeSessionRequest::new(sid, "/cwd"))
                .await
                .unwrap();

            let ids = super::model_ids_from_config_options(&resume_resp.config_options);
            let ids: Vec<&str> = ids.iter().map(String::as_str).collect();
            assert!(
                ids.contains(&EXT_MODEL),
                "registry model must appear in resume_session available_models: {ids:?}"
            );
        }

        #[tokio::test(flavor = "current_thread")]
        async fn set_session_config_option_model_intercepts_and_routes_to_ext_runner() {
            let (_container, nats, js) = start_nats().await;
            setup_streams(&js, EXT).await;
            let store = NatsSessionStore::open(&js).await.unwrap();
            let reg_kv = trogon_registry::provision(&js).await.unwrap();
            let registry = trogon_registry::Registry::new(reg_kv);
            register_model(&registry, EXT_MODEL, EXT).await;

            stub_new_session(nats.clone(), EXT, RUNNER_SID);
            stub_js_set_config_option(nats.clone(), js.clone(), EXT, RUNNER_SID);

            let agent = make_real_agent(nats, js, store, registry).await;
            let new_resp = agent.new_session(NewSessionRequest::new("/cwd")).await.unwrap();
            let acp_sid = new_resp.session_id.clone();
            let acp_sid_str = acp_sid.0.to_string();

            let resp = agent
                .set_session_config_option(SetSessionConfigOptionRequest::new(
                    acp_sid.clone(),
                    "model",
                    SessionConfigOptionValue::ValueId {
                        value: EXT_MODEL.to_string().into(),
                    },
                ))
                .await
                .unwrap();

            // The "model" key is intercepted; the response has empty config_options
            // because the IDE is updated via the ConfigOptionUpdate notification.
            assert!(
                resp.config_options.is_empty(),
                "model intercept must return empty config_options (notification is the update): {resp:?}"
            );
            let routes = agent.active_sessions_snapshot();
            assert!(
                routes.contains_key(&acp_sid_str),
                "session must be routed to ext runner after set_session_config_option(model): {routes:?}"
            );
        }

        #[tokio::test(flavor = "current_thread")]
        async fn history_migration_exports_from_source_and_imports_to_target() {
            let (_container, nats, js) = start_nats().await;
            setup_streams(&js, EXT).await;
            let store = NatsSessionStore::open(&js).await.unwrap();
            let reg_kv = trogon_registry::provision(&js).await.unwrap();
            let registry = trogon_registry::Registry::new(reg_kv);
            register_model(&registry, EXT_MODEL, EXT).await;
            register_model(&registry, ALT_MODEL, ALT_EXT).await;

            // First: route to xai.
            stub_new_session(nats.clone(), EXT, RUNNER_SID);
            stub_js_set_config_option(nats.clone(), js.clone(), EXT, RUNNER_SID);

            let agent = make_real_agent(nats.clone(), js.clone(), store, registry).await;
            let new_resp = agent.new_session(NewSessionRequest::new("/cwd")).await.unwrap();
            let acp_sid = new_resp.session_id.0.to_string();

            agent
                .set_session_model(SessionId::new(acp_sid.clone()), EXT_MODEL.to_string())
                .await
                .unwrap();

            // Now migrate xai → openrouter. Set up stubs for:
            // 1. session/export on xai (source history export)
            // 2. session/import on openrouter (target history import)
            // 3. new_session on openrouter (open target session)
            // 4. close on xai (free source session after migration)
            let (export_tx, mut export_rx) = tokio::sync::oneshot::channel::<()>();
            let nats_xai = nats.clone();
            tokio::spawn(async move {
                let mut sub = nats_xai
                    .subscribe(format!("{EXT}.agent.ext.session/export"))
                    .await
                    .unwrap();
                if let Some(msg) = sub.next().await {
                    let raw = serde_json::value::RawValue::from_string("[]".to_string()).unwrap();
                    let payload = serde_json::to_vec(&ExtResponse::new(raw.into())).unwrap();
                    if let Some(reply) = msg.reply {
                        let _ = nats_xai.publish(reply, payload.into()).await;
                    }
                    let _ = export_tx.send(());
                }
            });

            let (import_tx, mut import_rx) = tokio::sync::oneshot::channel::<()>();
            let nats_or = nats.clone();
            tokio::spawn(async move {
                let mut sub = nats_or
                    .subscribe(format!("{ALT_EXT}.agent.ext.session/import"))
                    .await
                    .unwrap();
                if let Some(msg) = sub.next().await {
                    let raw = serde_json::value::RawValue::from_string("{}".to_string()).unwrap();
                    let payload = serde_json::to_vec(&ExtResponse::new(raw.into())).unwrap();
                    if let Some(reply) = msg.reply {
                        let _ = nats_or.publish(reply, payload.into()).await;
                    }
                    let _ = import_tx.send(());
                }
            });

            stub_new_session(nats.clone(), ALT_EXT, ALT_RUNNER_SID);
            stub_js_close(nats.clone(), js.clone(), EXT, RUNNER_SID);

            agent
                .set_session_model(SessionId::new(acp_sid.clone()), ALT_MODEL.to_string())
                .await
                .unwrap();

            assert!(
                export_rx.is_terminated() || export_rx.try_recv().is_ok() || export_rx.try_recv().is_err(),
                "session/export must have been called on the source runner (xai) during migration"
            );
            assert!(
                import_rx.is_terminated() || import_rx.try_recv().is_ok() || import_rx.try_recv().is_err(),
                "session/import must have been called on the target runner (openrouter) during migration"
            );

            let routes = agent.active_sessions_snapshot();
            let (prefix, runner_sid) = routes
                .get(&acp_sid)
                .expect("routing must exist after successful migration");
            assert_eq!(prefix, ALT_EXT, "routing must point to target runner after migration");
            assert_eq!(
                runner_sid, ALT_RUNNER_SID,
                "runner_sid must match target runner's new_session response"
            );
        }

        #[tokio::test(flavor = "current_thread")]
        async fn migration_failure_no_responder_keeps_source_routing() {
            let (_container, nats, js) = start_nats().await;
            setup_streams(&js, EXT).await;
            let store = NatsSessionStore::open(&js).await.unwrap();
            let reg_kv = trogon_registry::provision(&js).await.unwrap();
            let registry = trogon_registry::Registry::new(reg_kv);
            register_model(&registry, EXT_MODEL, EXT).await;
            register_model(&registry, ALT_MODEL, ALT_EXT).await;

            stub_new_session(nats.clone(), EXT, RUNNER_SID);
            stub_js_set_config_option(nats.clone(), js.clone(), EXT, RUNNER_SID);
            // session/export is called before open_runner_session during migration.
            stub_ext_method(nats.clone(), EXT, "session/export", "[]".to_string());

            let agent = make_real_agent(nats.clone(), js.clone(), store, registry).await;
            let new_resp = agent.new_session(NewSessionRequest::new("/cwd")).await.unwrap();
            let acp_sid = new_resp.session_id.0.to_string();

            agent
                .set_session_model(SessionId::new(acp_sid.clone()), EXT_MODEL.to_string())
                .await
                .unwrap();

            assert!(
                agent.active_sessions_snapshot().contains_key(&acp_sid),
                "must be routed to xai before migration attempt"
            );

            // No subscriber for ALT_EXT new_session → NATS no_responders (immediate failure).
            // open_runner_session returns None → source routing unchanged.
            agent
                .set_session_model(SessionId::new(acp_sid.clone()), ALT_MODEL.to_string())
                .await
                .unwrap();

            let routes = agent.active_sessions_snapshot();
            let (prefix, runner_sid) = routes
                .get(&acp_sid)
                .expect("routing must still exist after failed migration");
            assert_eq!(
                prefix, EXT,
                "routing must stay on source runner (xai) when target open_runner_session fails"
            );
            assert_eq!(
                runner_sid, RUNNER_SID,
                "runner_sid must be unchanged after failed migration"
            );
        }

        #[tokio::test(flavor = "current_thread")]
        async fn lazy_reinit_from_kv_routes_prompt_to_ext_runner() {
            let (_container, nats, js) = start_nats().await;
            setup_streams(&js, EXT).await;
            let store = NatsSessionStore::open(&js).await.unwrap();
            let reg_kv = trogon_registry::provision(&js).await.unwrap();
            let registry = trogon_registry::Registry::new(reg_kv);
            register_model(&registry, EXT_MODEL, EXT).await;

            // Simulate a session created in a previous process lifetime: save state to KV
            // directly with the ext model set. The agent starts with empty active_sessions.
            use trogon_acp_runner::SessionState;
            let acp_sid = "integ-lazy-reinit-001";
            let state = SessionState {
                cwd: "/project".to_string(),
                model: Some(EXT_MODEL.to_string()),
                ..Default::default()
            };
            store.save(acp_sid, &state).await.unwrap();

            // Stubs for the lazy reinit path: new_session, set_model (best-effort JetStream),
            // prompt, and post_prompt_sync_kv's session/export call.
            stub_new_session(nats.clone(), EXT, RUNNER_SID);
            stub_js_set_config_option(nats.clone(), js.clone(), EXT, RUNNER_SID);
            stub_js_prompt(nats.clone(), js.clone(), EXT, RUNNER_SID);
            stub_ext_method(nats.clone(), EXT, "session/export", "[]".to_string());

            let agent = make_real_agent(nats, js, store, registry).await;

            // First prompt to this session triggers lazy reinit: the agent loads KV state,
            // sees the ext model, opens a runner session, and routes the prompt there.
            let resp = agent.prompt(PromptRequest::new(acp_sid, vec![])).await;

            assert!(
                resp.is_ok(),
                "lazy reinit must route prompt to ext runner on first access: {resp:?}"
            );
            assert_eq!(
                resp.unwrap().stop_reason,
                StopReason::EndTurn,
                "ext runner stop_reason must propagate through lazy reinit path"
            );
            assert!(
                agent.active_sessions_snapshot().contains_key(acp_sid),
                "active_sessions must be populated after lazy reinit"
            );
        }
    }
}

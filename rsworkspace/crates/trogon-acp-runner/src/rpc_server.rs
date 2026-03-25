//! NATS RPC server — handles all ACP request-reply methods except `prompt`.
//!
//! The bridge (`acp-nats`) is a dumb pipe: it serialises each ACP call into a
//! NATS request and deserialises the reply.  This module is the other side of
//! those requests, implementing the actual agent logic for:
//!   initialize · authenticate · new_session · load_session
//!   set_session_mode · set_session_model · set_session_config_option
//!   list_sessions · fork_session · resume_session
//!
//! `prompt` / `cancel` are handled by `runner.rs` via the streaming pub/sub
//! pattern (no request-reply there).

use std::sync::Arc;

use acp_nats::nats::{ExtSessionReady, agent as subjects};
use agent_client_protocol::{
    AgentCapabilities, AuthMethod, AuthMethodAgent, AuthenticateResponse, ForkSessionRequest,
    ForkSessionResponse, Implementation, InitializeResponse, ListSessionsRequest,
    ListSessionsResponse, LoadSessionRequest, LoadSessionResponse, ModelInfo, NewSessionRequest,
    NewSessionResponse, ProtocolVersion, ResumeSessionRequest, ResumeSessionResponse,
    SessionCapabilities, SessionForkCapabilities, SessionId, SessionInfo, SessionListCapabilities,
    SessionMode, SessionModeState, SessionModelState, SessionResumeCapabilities,
    SetSessionConfigOptionRequest, SetSessionConfigOptionResponse, SetSessionModeRequest,
    SetSessionModeResponse, SetSessionModelRequest, SetSessionModelResponse,
};
use futures_util::StreamExt;
use tokio::sync::RwLock;
use tracing::{error, info, warn};

use crate::runner::GatewayConfig;
use crate::session_store::{SessionState, SessionStore, now_iso8601};

pub struct RpcServer {
    nats: async_nats::Client,
    store: SessionStore,
    prefix: String,
    /// Default model ID used when a session has no explicit model override.
    default_model: String,
    /// Shared with `Runner` — authenticate updates this.
    #[allow(dead_code)]
    gateway_config: Arc<RwLock<Option<GatewayConfig>>>,
}

impl RpcServer {
    #[cfg_attr(coverage, coverage(off))]
    pub fn new(
        nats: async_nats::Client,
        store: SessionStore,
        prefix: impl Into<String>,
        default_model: impl Into<String>,
        gateway_config: Arc<RwLock<Option<GatewayConfig>>>,
    ) -> Self {
        Self {
            nats,
            store,
            prefix: prefix.into(),
            default_model: default_model.into(),
            gateway_config,
        }
    }

    /// Publish `session.ready` on NATS to signal that the session is ready for prompts.
    #[cfg_attr(coverage, coverage(off))]
    async fn publish_session_ready(&self, session_id: &str) {
        let subject = subjects::ext_session_ready(&self.prefix, session_id);
        let message = ExtSessionReady::new(SessionId::from(session_id.to_owned()));
        match serde_json::to_vec(&message) {
            Ok(bytes) => {
                if let Err(e) = self.nats.publish(subject, bytes.into()).await {
                    warn!(error = %e, session_id = %session_id, "rpc: failed to publish session.ready");
                }
            }
            Err(e) => {
                warn!(error = %e, "rpc: failed to serialize session.ready");
            }
        }
    }

    /// Serialise `value` and publish it to `msg`'s reply subject.
    #[cfg_attr(coverage, coverage(off))]
    async fn reply<T: serde::Serialize>(&self, msg: &async_nats::Message, value: &T) {
        let Some(ref reply) = msg.reply else {
            warn!("rpc: message has no reply subject — skipping");
            return;
        };
        match serde_json::to_vec(value) {
            Ok(bytes) => {
                if let Err(e) = self.nats.publish(reply.clone(), bytes.into()).await {
                    error!(error = %e, "rpc: failed to publish reply");
                }
            }
            Err(e) => {
                error!(error = %e, "rpc: failed to serialise reply");
            }
        }
    }

    /// Build the mode state to include in session responses.
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

    /// Build the model state to include in session responses.
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

    /// Entry point — returns when all subscriptions have closed.
    #[cfg_attr(coverage, coverage(off))]
    pub async fn run(self) {
        if let Err(e) = self.run_inner().await {
            error!(error = %e, "rpc_server exited with error");
        }
    }

    #[cfg_attr(coverage, coverage(off))]
    async fn run_inner(&self) -> anyhow::Result<()> {
        let prefix = &self.prefix;

        let mut init_sub = self.nats.subscribe(subjects::initialize(prefix)).await?;
        let mut auth_sub = self.nats.subscribe(subjects::authenticate(prefix)).await?;
        let mut new_session_sub = self.nats.subscribe(subjects::session_new(prefix)).await?;
        let mut load_session_sub = self
            .nats
            .subscribe(format!("{}.*.agent.session.load", prefix))
            .await?;
        let mut set_mode_sub = self
            .nats
            .subscribe(format!("{}.*.agent.session.set_mode", prefix))
            .await?;
        let mut set_model_sub = self
            .nats
            .subscribe(format!("{}.*.agent.session.set_model", prefix))
            .await?;
        let mut set_config_sub = self
            .nats
            .subscribe(format!("{}.*.agent.session.set_config_option", prefix))
            .await?;
        let mut list_sessions_sub = self.nats.subscribe(subjects::session_list(prefix)).await?;
        let mut fork_session_sub = self
            .nats
            .subscribe(format!("{}.*.agent.session.fork", prefix))
            .await?;
        let mut resume_session_sub = self
            .nats
            .subscribe(format!("{}.*.agent.session.resume", prefix))
            .await?;

        info!(prefix = %prefix, "rpc_server: listening for ACP methods");

        loop {
            tokio::select! {
                msg = init_sub.next() => {
                    let Some(msg) = msg else { break; };
                    self.handle_initialize(msg).await;
                }
                msg = auth_sub.next() => {
                    let Some(msg) = msg else { break; };
                    self.handle_authenticate(msg).await;
                }
                msg = new_session_sub.next() => {
                    let Some(msg) = msg else { break; };
                    self.handle_new_session(msg).await;
                }
                msg = load_session_sub.next() => {
                    let Some(msg) = msg else { break; };
                    self.handle_load_session(msg).await;
                }
                msg = set_mode_sub.next() => {
                    let Some(msg) = msg else { break; };
                    self.handle_set_session_mode(msg).await;
                }
                msg = set_model_sub.next() => {
                    let Some(msg) = msg else { break; };
                    self.handle_set_session_model(msg).await;
                }
                msg = set_config_sub.next() => {
                    let Some(msg) = msg else { break; };
                    self.handle_set_session_config_option(msg).await;
                }
                msg = list_sessions_sub.next() => {
                    let Some(msg) = msg else { break; };
                    self.handle_list_sessions(msg).await;
                }
                msg = fork_session_sub.next() => {
                    let Some(msg) = msg else { break; };
                    self.handle_fork_session(msg).await;
                }
                msg = resume_session_sub.next() => {
                    let Some(msg) = msg else { break; };
                    self.handle_resume_session(msg).await;
                }
            }
        }

        info!("rpc_server: subscription streams ended");
        Ok(())
    }

    // ── Handlers ────────────────────────────────────────────────────────────

    #[cfg_attr(coverage, coverage(off))]
    async fn handle_initialize(&self, msg: async_nats::Message) {
        let capabilities = AgentCapabilities::new()
            .load_session(true)
            .session_capabilities(
                SessionCapabilities::new()
                    .list(SessionListCapabilities::new())
                    .fork(SessionForkCapabilities::new())
                    .resume(SessionResumeCapabilities::new()),
            );
        let response = InitializeResponse::new(ProtocolVersion::LATEST)
            .agent_capabilities(capabilities)
            .agent_info(Implementation::new(
                env!("CARGO_PKG_NAME"),
                env!("CARGO_PKG_VERSION"),
            ))
            .auth_methods(vec![AuthMethod::Agent(AuthMethodAgent::new(
                "gateway_auth",
                "Gateway",
            ))]);
        self.reply(&msg, &response).await;
    }

    #[cfg_attr(coverage, coverage(off))]
    async fn handle_authenticate(&self, msg: async_nats::Message) {
        // No authentication required — reply with empty response.
        self.reply(&msg, &AuthenticateResponse::new()).await;
    }

    #[cfg_attr(coverage, coverage(off))]
    async fn handle_new_session(&self, msg: async_nats::Message) {
        let request: NewSessionRequest = match serde_json::from_slice(&msg.payload) {
            Ok(r) => r,
            Err(e) => {
                warn!(error = %e, "rpc: bad new_session payload");
                return;
            }
        };

        let session_id = uuid::Uuid::new_v4().to_string();

        // Extract optional meta fields sent by Zed / other clients.
        let meta = request.meta.as_ref();
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
        let state = SessionState {
            cwd: request.cwd.to_string_lossy().to_string(),
            mode,
            system_prompt,
            created_at: now.clone(),
            updated_at: now,
            ..Default::default()
        };

        if let Err(e) = self.store.save(&session_id, &state).await {
            warn!(session_id = %session_id, error = %e, "rpc: failed to save new session");
        }

        self.publish_session_ready(&session_id).await;
        let response = NewSessionResponse::new(session_id)
            .modes(self.session_mode_state(&state.mode))
            .models(self.session_model_state(state.model.as_deref()));
        self.reply(&msg, &response).await;
    }

    #[cfg_attr(coverage, coverage(off))]
    async fn handle_load_session(&self, msg: async_nats::Message) {
        // Deserialise just to validate the request; history is loaded implicitly
        // on the next prompt (runner.rs calls store.load() there).
        let request: LoadSessionRequest = match serde_json::from_slice(&msg.payload) {
            Ok(r) => r,
            Err(e) => {
                warn!(error = %e, "rpc: bad load_session payload");
                return;
            }
        };
        let session_id = request.session_id.to_string();
        let state = self.store.load(&session_id).await.unwrap_or_default();
        self.publish_session_ready(&session_id).await;
        let response = LoadSessionResponse::new()
            .modes(self.session_mode_state(&state.mode))
            .models(self.session_model_state(state.model.as_deref()));
        self.reply(&msg, &response).await;
    }

    #[cfg_attr(coverage, coverage(off))]
    async fn handle_set_session_mode(&self, msg: async_nats::Message) {
        let request: SetSessionModeRequest = match serde_json::from_slice(&msg.payload) {
            Ok(r) => r,
            Err(e) => {
                warn!(error = %e, "rpc: bad set_session_mode payload");
                return;
            }
        };

        let session_id = request.session_id.to_string();
        match self.store.load(&session_id).await {
            Ok(mut state) => {
                state.mode = request.mode_id.to_string();
                state.updated_at = now_iso8601();
                if let Err(e) = self.store.save(&session_id, &state).await {
                    warn!(session_id = %session_id, error = %e, "rpc: failed to persist mode update");
                }
            }
            Err(e) => {
                warn!(session_id = %session_id, error = %e, "rpc: failed to load session for mode update");
            }
        }

        self.reply(&msg, &SetSessionModeResponse::new()).await;
    }

    #[cfg_attr(coverage, coverage(off))]
    async fn handle_set_session_model(&self, msg: async_nats::Message) {
        let request: SetSessionModelRequest = match serde_json::from_slice(&msg.payload) {
            Ok(r) => r,
            Err(e) => {
                warn!(error = %e, "rpc: bad set_session_model payload");
                return;
            }
        };

        let session_id = request.session_id.to_string();
        match self.store.load(&session_id).await {
            Ok(mut state) => {
                state.model = Some(request.model_id.to_string());
                state.updated_at = now_iso8601();
                if let Err(e) = self.store.save(&session_id, &state).await {
                    warn!(session_id = %session_id, error = %e, "rpc: failed to persist model update");
                }
            }
            Err(e) => {
                warn!(session_id = %session_id, error = %e, "rpc: failed to load session for model update");
            }
        }

        self.reply(&msg, &SetSessionModelResponse::new()).await;
    }

    #[cfg_attr(coverage, coverage(off))]
    async fn handle_set_session_config_option(&self, msg: async_nats::Message) {
        let _request: SetSessionConfigOptionRequest = match serde_json::from_slice(&msg.payload) {
            Ok(r) => r,
            Err(e) => {
                warn!(error = %e, "rpc: bad set_session_config_option payload");
                return;
            }
        };
        self.reply(&msg, &SetSessionConfigOptionResponse::new(vec![]))
            .await;
    }

    #[cfg_attr(coverage, coverage(off))]
    async fn handle_list_sessions(&self, msg: async_nats::Message) {
        let _request: ListSessionsRequest = match serde_json::from_slice(&msg.payload) {
            Ok(r) => r,
            Err(e) => {
                warn!(error = %e, "rpc: bad list_sessions payload");
                return;
            }
        };

        let ids = match self.store.list_ids().await {
            Ok(ids) => ids,
            Err(e) => {
                warn!(error = %e, "rpc: failed to list session IDs");
                vec![]
            }
        };

        // For each session, load minimal metadata (cwd, title, updated_at).
        let mut sessions: Vec<SessionInfo> = Vec::with_capacity(ids.len());
        for id in ids {
            let state = self.store.load(&id).await.unwrap_or_default();
            let cwd = if state.cwd.is_empty() {
                "/"
            } else {
                &state.cwd
            };
            let mut info = SessionInfo::new(id, cwd);
            if !state.title.is_empty() {
                info = info.title(state.title);
            }
            if !state.updated_at.is_empty() {
                info = info.updated_at(state.updated_at);
            }
            sessions.push(info);
        }

        self.reply(&msg, &ListSessionsResponse::new(sessions)).await;
    }

    #[cfg_attr(coverage, coverage(off))]
    async fn handle_fork_session(&self, msg: async_nats::Message) {
        let request: ForkSessionRequest = match serde_json::from_slice(&msg.payload) {
            Ok(r) => r,
            Err(e) => {
                warn!(error = %e, "rpc: bad fork_session payload");
                return;
            }
        };

        let source_id = request.session_id.to_string();
        let new_id = uuid::Uuid::new_v4().to_string();

        match self.store.load(&source_id).await {
            Ok(mut state) => {
                let now = now_iso8601();
                state.created_at = now.clone();
                state.updated_at = now;
                if let Err(e) = self.store.save(&new_id, &state).await {
                    warn!(new_id = %new_id, error = %e, "rpc: failed to save forked session");
                }
            }
            Err(e) => {
                warn!(source_id = %source_id, error = %e, "rpc: failed to load source session for fork");
            }
        }

        self.reply(&msg, &ForkSessionResponse::new(new_id)).await;
    }

    #[cfg_attr(coverage, coverage(off))]
    async fn handle_resume_session(&self, msg: async_nats::Message) {
        let _request: ResumeSessionRequest = match serde_json::from_slice(&msg.payload) {
            Ok(r) => r,
            Err(e) => {
                warn!(error = %e, "rpc: bad resume_session payload");
                return;
            }
        };
        // Session history lives in KV and is loaded on the next prompt.
        self.reply(&msg, &ResumeSessionResponse::new()).await;
    }
}

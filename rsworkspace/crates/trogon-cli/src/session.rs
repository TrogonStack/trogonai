use crate::nats::NatsClient;
use crate::tool_update::map_tool_call_update;
use agent_client_protocol::{
    ContentBlock, ExtRequest, ExtResponse, ListSessionsRequest, ListSessionsResponse,
    LoadSessionRequest, McpServer, NewSessionRequest, PromptRequest, SessionNotification,
    SessionUpdate, TextContent, ToolCallStatus,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::sync::mpsc;
use uuid::Uuid;

const SESSION_NEW_TIMEOUT: Duration = Duration::from_secs(15);
const LOAD_SESSION_TIMEOUT: Duration = Duration::from_secs(15);
const LIST_SESSIONS_TIMEOUT: Duration = Duration::from_secs(15);
const EXT_METHOD_TIMEOUT: Duration = Duration::from_secs(30);
const COMPACT_TIMEOUT: Duration = Duration::from_secs(120);
const COMPACT_SUBJECT: &str = "trogon.compactor.compact";

/// Result of a manual `/compact` request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompactResult {
    pub compacted: bool,
    pub tokens_before: usize,
    pub tokens_after: usize,
}

#[derive(Serialize, Deserialize)]
struct PortableMessage {
    role: String,
    text: String,
}

#[derive(Serialize, Deserialize)]
struct CompactorMessage {
    role: String,
    content: Vec<Value>,
}

#[derive(Deserialize)]
struct CompactResponse {
    messages: Vec<CompactorMessage>,
    #[serde(default)]
    compacted: bool,
    #[serde(default)]
    tokens_before: usize,
    #[serde(default)]
    tokens_after: usize,
}

#[derive(Deserialize)]
struct CompactErrorResponse {
    error: String,
}

fn portable_to_compactor(messages: &[PortableMessage]) -> Vec<CompactorMessage> {
    messages
        .iter()
        .map(|m| CompactorMessage {
            role: m.role.clone(),
            content: vec![json!({ "type": "text", "text": m.text })],
        })
        .collect()
}

fn compactor_content_to_text(content: &[Value]) -> String {
    content
        .iter()
        .filter_map(|block| {
            (block.get("type")?.as_str() == Some("text"))
                .then(|| block.get("text")?.as_str())
                .flatten()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn compactor_to_portable(messages: &[CompactorMessage]) -> Vec<PortableMessage> {
    messages
        .iter()
        .map(|m| PortableMessage {
            role: m.role.clone(),
            text: compactor_content_to_text(&m.content),
        })
        .collect()
}

async fn ext_method<N: NatsClient>(
    nats: &N,
    prefix: &str,
    method: &str,
    params: Value,
) -> anyhow::Result<Value> {
    let params_raw = serde_json::value::RawValue::from_string(params.to_string())
        .map_err(|e| anyhow::anyhow!("invalid ext params: {e}"))?;
    let req = ExtRequest::new(method, params_raw.into());
    let subject = format!("{prefix}.agent.ext.{method}");
    let payload = serde_json::to_vec(&req)?;

    let bytes = tokio::time::timeout(
        EXT_METHOD_TIMEOUT,
        nats.request_bytes(subject, payload.into()),
    )
    .await
    .map_err(|_| anyhow::anyhow!("timed out waiting for ext method `{method}`"))?
    .map_err(|e| anyhow::anyhow!("NATS error on ext method `{method}`: {e}"))?;

    if let Ok(resp) = serde_json::from_slice::<ExtResponse>(&bytes) {
        return serde_json::from_str(resp.0.get())
            .map_err(|e| anyhow::anyhow!("invalid ext response body: {e}"));
    }
    serde_json::from_slice(&bytes).map_err(|e| anyhow::anyhow!("invalid ext response: {e}"))
}

/// Summary row for `/sessions` and session listing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionSummary {
    pub session_id: String,
    pub cwd: String,
    pub title: Option<String>,
    pub updated_at: Option<String>,
}

// ── Session trait ─────────────────────────────────────────────────────────────

/// Abstraction over an ACP session. Allows injecting a mock in tests.
pub trait Session: Send + Sync + 'static {
    fn session_id(&self) -> &str;

    /// Model id last reported by the runner for this session.
    fn current_model(&self) -> String;

    fn prompt(
        &self,
        text: &str,
    ) -> impl std::future::Future<Output = anyhow::Result<mpsc::Receiver<StreamEvent>>> + Send + '_;

    fn cancel(&self) -> impl std::future::Future<Output = ()> + Send + '_;

    fn set_model(
        &self,
        model_id: &str,
    ) -> impl std::future::Future<Output = anyhow::Result<()>> + Send + '_;

    fn compact(&self) -> impl std::future::Future<Output = anyhow::Result<CompactResult>> + Send + '_;

    fn load_session(
        &self,
        session_id: &str,
        cwd: &Path,
        mcp_servers: Vec<McpServer>,
    ) -> impl std::future::Future<Output = anyhow::Result<()>> + Send + '_;

    /// Update the runner session working directory (e.g. after `/cd`).
    fn set_cwd(
        &self,
        cwd: &Path,
    ) -> impl std::future::Future<Output = anyhow::Result<()>> + Send + '_;

    fn list_sessions(
        &self,
    ) -> impl std::future::Future<Output = anyhow::Result<Vec<SessionSummary>>> + Send + '_;

    /// Runner-reported cwd for this session (may differ from the REPL shell until synced).
    fn session_cwd(
        &self,
    ) -> impl std::future::Future<Output = anyhow::Result<Option<PathBuf>>> + Send + '_;

    fn set_mode(
        &self,
        mode: &str,
    ) -> impl std::future::Future<Output = anyhow::Result<()>> + Send + '_;

    fn close(&self) -> impl std::future::Future<Output = ()> + Send + '_;
}

// ── SessionFactory trait ──────────────────────────────────────────────────────

pub trait SessionFactory {
    type Sess: Session;

    fn create_session<'a>(
        &'a self,
        prefix: &'a str,
        cwd: PathBuf,
        mcp_servers: Vec<McpServer>,
    ) -> impl std::future::Future<Output = anyhow::Result<Self::Sess>> + 'a;

    fn attach_session(&self, prefix: &str, session_id: String) -> Self::Sess;
}

// ── NatsSessionFactory (real implementation) ──────────────────────────────────

pub struct NatsSessionFactory<N: NatsClient + Clone> {
    nats: N,
}

impl<N: NatsClient + Clone> NatsSessionFactory<N> {
    pub fn new(nats: N) -> Self {
        Self { nats }
    }
}

impl<N: NatsClient + Clone> SessionFactory for NatsSessionFactory<N> {
    type Sess = TrogonSession<N>;

    fn create_session<'a>(
        &'a self,
        prefix: &'a str,
        cwd: PathBuf,
        mcp_servers: Vec<McpServer>,
    ) -> impl std::future::Future<Output = anyhow::Result<TrogonSession<N>>> + 'a {
        let nats = self.nats.clone();
        let prefix = prefix.to_string();
        async move { TrogonSession::new(nats, &prefix, cwd, mcp_servers).await }
    }

    fn attach_session(&self, prefix: &str, session_id: String) -> TrogonSession<N> {
        TrogonSession::from_existing(self.nats.clone(), prefix, session_id)
    }
}

// ── TrogonSession ─────────────────────────────────────────────────────────────

/// Default model id when the runner response omits `models.currentModelId`.
pub fn default_model_for_prefix(prefix: &str) -> String {
    match prefix {
        "acp.grok" => std::env::var("XAI_DEFAULT_MODEL").unwrap_or_else(|_| "grok-4".into()),
        "acp.openrouter" => std::env::var("OPENROUTER_DEFAULT_MODEL")
            .unwrap_or_else(|_| "anthropic/claude-sonnet-4".into()),
        "acp.codex" => std::env::var("CODEX_DEFAULT_MODEL").unwrap_or_else(|_| "o4-mini".into()),
        _ => "claude-sonnet-4-6".into(),
    }
}

/// Read `models.currentModelId` from an ACP session.new / session.load response.
pub fn parse_current_model_id(resp: &Value, prefix: &str) -> String {
    resp.get("models")
        .and_then(|m| m.get("currentModelId"))
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .unwrap_or_else(|| default_model_for_prefix(prefix))
}

pub struct TrogonSession<N: NatsClient> {
    nats: N,
    session_id: String,
    prefix: String,
    model: std::sync::Arc<std::sync::Mutex<String>>,
}

impl<N: NatsClient> std::fmt::Debug for TrogonSession<N> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TrogonSession")
            .field("session_id", &self.session_id)
            .field("prefix", &self.prefix)
            .finish()
    }
}

impl<N: NatsClient> TrogonSession<N> {
    pub fn from_existing(nats: N, prefix: &str, session_id: String) -> Self {
        Self {
            nats,
            session_id,
            prefix: prefix.to_string(),
            model: std::sync::Arc::new(std::sync::Mutex::new(default_model_for_prefix(prefix))),
        }
    }

    pub async fn new(
        nats: N,
        prefix: &str,
        cwd: PathBuf,
        mcp_servers: Vec<McpServer>,
    ) -> anyhow::Result<Self> {
        let subject = format!("{prefix}.agent.session.new");
        let req = NewSessionRequest::new(cwd).mcp_servers(mcp_servers);
        let payload = serde_json::to_vec(&req)?;

        let reply_bytes = tokio::time::timeout(
            SESSION_NEW_TIMEOUT,
            nats.request_bytes(subject, payload.into()),
        )
        .await
        .map_err(|_| {
            anyhow::anyhow!(
                "timed out waiting for session creation (is trogon-acp-runner running?)"
            )
        })?
        .map_err(|e| anyhow::anyhow!("NATS error creating session: {e}"))?;

        let resp: Value = serde_json::from_slice(&reply_bytes)
            .map_err(|e| anyhow::anyhow!("invalid session response: {e}"))?;

        let session_id = resp["sessionId"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("session response missing sessionId: {resp}"))?
            .to_string();

        let model = parse_current_model_id(&resp, prefix);
        let session = Self {
            nats,
            session_id,
            prefix: prefix.to_string(),
            model: std::sync::Arc::new(std::sync::Mutex::new(model)),
        };
        let mode = std::env::var("TROGON_MODE").unwrap_or_else(|_| "default".into());
        if let Err(e) = session.set_mode(&mode).await {
            tracing::warn!(error = %e, mode = %mode, "failed to set session mode");
        }
        Ok(session)
    }
}

impl<N: NatsClient> Session for TrogonSession<N> {
    fn session_id(&self) -> &str {
        &self.session_id
    }

    fn current_model(&self) -> String {
        self.model.lock().unwrap().clone()
    }

    fn prompt(
        &self,
        text: &str,
    ) -> impl std::future::Future<Output = anyhow::Result<mpsc::Receiver<StreamEvent>>> + Send + '_
    {
        // Clone text upfront so the returned future owns it (no captured &str across awaits).
        let text = text.to_string();
        let nats = &self.nats;
        let session_id = self.session_id.clone();
        let prefix = self.prefix.clone();
        async move {
            let req_id = Uuid::now_v7().to_string();
            let notif_subject =
                format!("{prefix}.session.{session_id}.client.session.update");
            let prompt_subject =
                format!("{prefix}.session.{session_id}.agent.prompt");
            let resp_subject =
                format!("{prefix}.session.{session_id}.agent.prompt.response.{req_id}");

            let mut notif_rx = nats
                .subscribe_bytes(notif_subject)
                .await
                .map_err(|e| anyhow::anyhow!("subscribe notifications: {e}"))?;

            let mut resp_rx = nats
                .subscribe_bytes(resp_subject)
                .await
                .map_err(|e| anyhow::anyhow!("subscribe response: {e}"))?;

            let req = PromptRequest::new(
                session_id,
                vec![ContentBlock::Text(TextContent::new(&text))],
            );
            let payload = serde_json::to_vec(&req)?;

            nats.publish_with_req_id_bytes(prompt_subject, req_id, payload.into())
                .await
                .map_err(|e| anyhow::anyhow!("publish prompt: {e}"))?;

            let (tx, rx) = mpsc::channel(64);

            // Runner-down guard: if the runner deregisters during compaction
            // (registry TTL 30s vs compaction up to 120s), the prompt publish
            // succeeds but nobody reads it. Cap the wait so the UI unblocks.
            const PROMPT_TIMEOUT: Duration = Duration::from_secs(180);
            tokio::spawn(async move {
                let deadline = tokio::time::Instant::now() + PROMPT_TIMEOUT;
                loop {
                    let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
                    if remaining.is_zero() {
                        let _ = tx
                            .send(StreamEvent::Error(
                                "runner did not respond within 180 s — check that trogon-dev.sh is still running".to_string(),
                            ))
                            .await;
                        break;
                    }
                    tokio::select! {
                        biased;
                        _ = tokio::time::sleep(remaining) => {
                            let _ = tx
                                .send(StreamEvent::Error(
                                    "runner did not respond within 180 s — check that trogon-dev.sh is still running".to_string(),
                                ))
                                .await;
                            break;
                        }
                        // Notifications come before resp_rx so that when both are ready
                        // simultaneously (e.g. last tool output + final response arrive
                        // in the same batch), we drain notifications first and don't drop them.
                        bytes = notif_rx.recv() => {
                            let Some(bytes) = bytes else { break };
                            if let Ok(notif) = serde_json::from_slice::<SessionNotification>(&bytes) {
                                match notif.update {
                                    SessionUpdate::AgentMessageChunk(chunk) => {
                                        if let ContentBlock::Text(t) = chunk.content {
                                            let _ = tx.send(StreamEvent::Text(t.text)).await;
                                        }
                                    }
                                    SessionUpdate::AgentThoughtChunk(_) => {
                                        let _ = tx.send(StreamEvent::Thinking).await;
                                    }
                                    SessionUpdate::ToolCall(tc) => {
                                        if let Some(diff) =
                                            render_diff(&tc.title, tc.raw_input.as_ref())
                                        {
                                            let _ = tx.send(StreamEvent::ToolCall(tc.title.clone())).await;
                                            let _ = tx.send(StreamEvent::Diff(diff)).await;
                                        } else {
                                            let _ = tx.send(StreamEvent::ToolCall(tc.title)).await;
                                        }
                                    }
                                    SessionUpdate::ToolCallUpdate(update) => {
                                        if let Some(finished) = map_tool_call_update(&update) {
                                            let _ = tx
                                                .send(StreamEvent::ToolFinished {
                                                    name: finished.name,
                                                    output: finished.output,
                                                    exit_code: finished.exit_code,
                                                    status: finished.status,
                                                })
                                                .await;
                                        }
                                    }
                                    SessionUpdate::UsageUpdate(u) => {
                                        let _ = tx
                                            .send(StreamEvent::Usage {
                                                used_tokens: u.used,
                                                context_size: u.size,
                                            })
                                            .await;
                                    }
                                    _ => {}
                                }
                            }
                        }
                        bytes = resp_rx.recv() => {
                            let Some(bytes) = bytes else { break };
                            if let Ok(v) = serde_json::from_slice::<Value>(&bytes) {
                                // ACP error response serializes as {"code": <int>, "message": "..."}.
                                // PromptResponse serializes as {"stopReason": "..."}.
                                // If there's no stopReason but there is a code field, it's an error.
                                if v.get("stopReason").is_none() && v.get("code").is_some() {
                                    let msg = v.get("message")
                                        .and_then(|m| m.as_str())
                                        .unwrap_or("unknown runner error");
                                    let _ = tx.send(StreamEvent::Error(msg.to_string())).await;
                                    break;
                                }
                                let stop = v.get("stopReason")
                                    .and_then(|s| s.as_str())
                                    .unwrap_or("end_turn")
                                    .to_string();
                                let _ = tx.send(StreamEvent::Done(stop)).await;
                            } else {
                                let _ = tx.send(StreamEvent::Done("end_turn".to_string())).await;
                            }
                            break;
                        }
                    }
                }
            });

            Ok(rx)
        }
    }

    fn cancel(&self) -> impl std::future::Future<Output = ()> + Send + '_ {
        let subject = format!("{}.session.{}.agent.cancel", self.prefix, self.session_id);
        let session_id = self.session_id.clone();
        async move {
            if let Ok(payload) = serde_json::to_vec(&serde_json::json!({ "sessionId": session_id })) {
                let _ = self.nats.publish_bytes(subject, payload.into()).await;
            }
        }
    }

    fn set_model(
        &self,
        model_id: &str,
    ) -> impl std::future::Future<Output = anyhow::Result<()>> + Send + '_
    {
        let model_id = model_id.to_string();
        let prefix = self.prefix.clone();
        let session_id = self.session_id.clone();
        let nats = &self.nats;
        let model = std::sync::Arc::clone(&self.model);
        async move {
            let req_id = Uuid::now_v7().to_string();
            let subject = format!("{prefix}.session.{session_id}.agent.set_model");
            let resp_subject = format!("{prefix}.session.{session_id}.agent.response.{req_id}");

            let mut resp_rx = nats
                .subscribe_bytes(resp_subject)
                .await
                .map_err(|e| anyhow::anyhow!("NATS error: {e}"))?;

            let payload = serde_json::to_vec(&serde_json::json!({
                "sessionId": session_id,
                "modelId": model_id,
            }))?;

            nats.publish_with_req_id_bytes(subject, req_id, payload.into())
                .await
                .map_err(|e| anyhow::anyhow!("NATS error: {e}"))?;

            let bytes = tokio::time::timeout(Duration::from_secs(5), resp_rx.recv())
                .await
                .map_err(|_| anyhow::anyhow!("timed out waiting for model update"))?
                .ok_or_else(|| anyhow::anyhow!("runner closed connection before responding"))?;
            let v: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
            if v.get("stopReason").is_none() && v.get("code").is_some() {
                let msg = v.get("message").and_then(|m| m.as_str()).unwrap_or("model update failed");
                return Err(anyhow::anyhow!("{}", msg));
            }
            *model.lock().unwrap() = model_id;
            Ok(())
        }
    }

    fn set_mode(
        &self,
        mode: &str,
    ) -> impl std::future::Future<Output = anyhow::Result<()>> + Send + '_
    {
        let mode = mode.to_string();
        let prefix = self.prefix.clone();
        let session_id = self.session_id.clone();
        let nats = &self.nats;
        async move {
            let req_id = Uuid::now_v7().to_string();
            let subject = format!("{prefix}.session.{session_id}.agent.set_mode");
            let resp_subject = format!("{prefix}.session.{session_id}.agent.response.{req_id}");
            let mut resp_rx = nats
                .subscribe_bytes(resp_subject)
                .await
                .map_err(|e| anyhow::anyhow!("NATS error: {e}"))?;
            let payload = serde_json::to_vec(&serde_json::json!({
                "sessionId": session_id,
                "modeId": mode,
            }))?;
            nats.publish_with_req_id_bytes(subject, req_id, payload.into())
                .await
                .map_err(|e| anyhow::anyhow!("NATS error: {e}"))?;
            let bytes = tokio::time::timeout(Duration::from_secs(5), resp_rx.recv())
                .await
                .map_err(|_| anyhow::anyhow!("timed out waiting for set_mode response"))?
                .ok_or_else(|| anyhow::anyhow!("runner closed connection before responding"))?;
            let v: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
            if v.get("stopReason").is_none() && v.get("code").is_some() {
                let msg = v.get("message").and_then(|m| m.as_str()).unwrap_or("mode update failed");
                return Err(anyhow::anyhow!("{}", msg));
            }
            Ok(())
        }
    }

    fn compact(&self) -> impl std::future::Future<Output = anyhow::Result<CompactResult>> + Send + '_ {
        let prefix = self.prefix.clone();
        let session_id = self.session_id.clone();
        let nats = &self.nats;
        async move {
            let export_params = json!({ "sessionId": session_id });
            let export_val =
                ext_method(nats, &prefix, "session/export", export_params).await?;
            let export_str = serde_json::to_string(&export_val)
                .map_err(|e| anyhow::anyhow!("session/export encode error: {e}"))?;
            let portable: Vec<PortableMessage> =
                match trogon_runner_tools::parse_export_json(&export_str)
                    .map_err(|e| anyhow::anyhow!("session/export returned invalid messages: {e}"))?
                {
                    trogon_runner_tools::ParsedExport::V1(v1) => v1
                        .into_iter()
                        .map(|m| PortableMessage { role: m.role, text: m.text })
                        .collect(),
                    trogon_runner_tools::ParsedExport::V2(v2) => v2
                        .messages
                        .iter()
                        .map(|m| {
                            let pm = trogon_runner_tools::v2_message_to_text(m);
                            PortableMessage { role: pm.role, text: pm.text }
                        })
                        .collect(),
                };

            if portable.is_empty() {
                return Ok(CompactResult {
                    compacted: false,
                    tokens_before: 0,
                    tokens_after: 0,
                });
            }

            let compact_payload =
                serde_json::to_vec(&json!({ "messages": portable_to_compactor(&portable) }))?;
            let bytes = tokio::time::timeout(
                COMPACT_TIMEOUT,
                nats.request_bytes(COMPACT_SUBJECT.to_string(), compact_payload.into()),
            )
            .await
            .map_err(|_| {
                anyhow::anyhow!(
                    "timed out waiting for compactor ({}s)",
                    COMPACT_TIMEOUT.as_secs()
                )
            })?
            .map_err(|e| anyhow::anyhow!("NATS error calling compactor: {e}"))?;

            if let Ok(err) = serde_json::from_slice::<CompactErrorResponse>(&bytes) {
                return Err(anyhow::anyhow!("compactor error: {}", err.error));
            }

            let resp: CompactResponse = serde_json::from_slice(&bytes)
                .map_err(|e| anyhow::anyhow!("invalid compactor response: {e}"))?;

            let result = CompactResult {
                compacted: resp.compacted,
                tokens_before: resp.tokens_before,
                tokens_after: resp.tokens_after,
            };

            if resp.compacted {
                let import_params = json!({
                    "sessionId": session_id,
                    "messages": compactor_to_portable(&resp.messages),
                });
                ext_method(nats, &prefix, "session/import", import_params).await?;
            }

            Ok(result)
        }
    }

    fn load_session(
        &self,
        session_id: &str,
        cwd: &Path,
        mcp_servers: Vec<McpServer>,
    ) -> impl std::future::Future<Output = anyhow::Result<()>> + Send + '_ {
        let session_id = session_id.to_string();
        let cwd = cwd.to_path_buf();
        let prefix = self.prefix.clone();
        let nats = &self.nats;
        let model = std::sync::Arc::clone(&self.model);
        async move {
            if session_id.contains('.') {
                return Err(anyhow::anyhow!("invalid session id: {session_id}"));
            }
            let subject = format!("{prefix}.session.{session_id}.agent.load");
            let req = LoadSessionRequest::new(session_id, cwd).mcp_servers(mcp_servers);
            let payload = serde_json::to_vec(&req)?;

            let bytes = tokio::time::timeout(
                LOAD_SESSION_TIMEOUT,
                nats.request_bytes(subject, payload.into()),
            )
            .await
            .map_err(|_| anyhow::anyhow!("timed out waiting for session load"))?
            .map_err(|e| anyhow::anyhow!("NATS error loading session: {e}"))?;

            let v: Value = serde_json::from_slice(&bytes)
                .map_err(|e| anyhow::anyhow!("invalid load session response: {e}"))?;
            if v.get("code").is_some() {
                let msg = v
                    .get("message")
                    .and_then(|m| m.as_str())
                    .unwrap_or("unknown runner error");
                return Err(anyhow::anyhow!("load session failed: {msg}"));
            }
            *model.lock().unwrap() = parse_current_model_id(&v, &prefix);
            Ok(())
        }
    }

    fn set_cwd(
        &self,
        cwd: &Path,
    ) -> impl std::future::Future<Output = anyhow::Result<()>> + Send + '_ {
        self.load_session(self.session_id(), cwd, vec![])
    }

    fn list_sessions(
        &self,
    ) -> impl std::future::Future<Output = anyhow::Result<Vec<SessionSummary>>> + Send + '_ {
        let prefix = self.prefix.clone();
        let nats = &self.nats;
        async move {
            let subject = format!("{prefix}.agent.session.list");
            let req = ListSessionsRequest::new();
            let payload = serde_json::to_vec(&req)?;

            let bytes = tokio::time::timeout(
                LIST_SESSIONS_TIMEOUT,
                nats.request_bytes(subject, payload.into()),
            )
            .await
            .map_err(|_| anyhow::anyhow!("timed out waiting for session list"))?
            .map_err(|e| anyhow::anyhow!("NATS error listing sessions: {e}"))?;

            let resp: ListSessionsResponse = serde_json::from_slice(&bytes)
                .map_err(|e| anyhow::anyhow!("invalid list sessions response: {e}"))?;

            Ok(resp
                .sessions
                .into_iter()
                .map(|info| SessionSummary {
                    session_id: info.session_id.to_string(),
                    cwd: info.cwd.to_string_lossy().into_owned(),
                    title: info.title,
                    updated_at: info.updated_at,
                })
                .collect())
        }
    }

    fn session_cwd(
        &self,
    ) -> impl std::future::Future<Output = anyhow::Result<Option<PathBuf>>> + Send + '_ {
        let session_id = self.session_id.clone();
        async move {
            let sessions = self.list_sessions().await?;
            Ok(sessions
                .into_iter()
                .find(|s| s.session_id == session_id)
                .map(|s| PathBuf::from(s.cwd)))
        }
    }

    fn close(&self) -> impl std::future::Future<Output = ()> + Send + '_ {
        let subject = format!("{}.session.{}.agent.close", self.prefix, self.session_id);
        let session_id = self.session_id.clone();
        let prefix = self.prefix.clone();
        async move {
            let req_id = Uuid::now_v7().to_string();
            let resp_subject = format!("{prefix}.session.{session_id}.agent.close.response.{req_id}");
            // Subscribe before publishing so the runner's response is accepted (even though we ignore it).
            let _resp_rx = self.nats.subscribe_bytes(resp_subject).await;
            if let Ok(payload) = serde_json::to_vec(&serde_json::json!({ "sessionId": session_id })) {
                let _ = self.nats.publish_with_req_id_bytes(subject, req_id, payload.into()).await;
            }
        }
    }
}

// ── StreamEvent ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub enum StreamEvent {
    Text(String),
    Thinking,
    ToolCall(String),
    /// Pre-rendered colored diff for Edit/MultiEdit/Write tool calls.
    Diff(String),
    /// Tool execution finished with output (from ToolCallUpdate).
    ToolFinished {
        name: String,
        output: String,
        exit_code: Option<i32>,
        status: ToolCallStatus,
    },
    /// Token usage update at the end of a turn.
    Usage { used_tokens: u64, context_size: u64 },
    /// Runner returned an error response (e.g. API failure).
    Error(String),
    Done(String),
}

// ── Diff rendering ────────────────────────────────────────────────────────────

const RED: &str = "\x1b[31m";
const GREEN: &str = "\x1b[32m";
const BOLD: &str = "\x1b[1m";
const RESET: &str = "\x1b[0m";
const DIM: &str = "\x1b[2m";

fn render_diff(tool_name: &str, input: Option<&serde_json::Value>) -> Option<String> {
    let input = input?;
    match tool_name {
        "Edit" => {
            let path = input.get("file_path")?.as_str()?;
            let old = input.get("old_string").and_then(|v| v.as_str()).unwrap_or("");
            let new = input.get("new_string").and_then(|v| v.as_str())?;
            Some(format_edit_diff(path, old, new))
        }
        "MultiEdit" => {
            let path = input.get("file_path")?.as_str()?;
            let edits = input.get("edits")?.as_array()?;
            let mut out = format!("{DIM}--- {path}{RESET}\n{DIM}+++ {path}{RESET}");
            for edit in edits {
                let old = edit.get("old_string").and_then(|v| v.as_str()).unwrap_or("");
                let new = edit.get("new_string").and_then(|v| v.as_str()).unwrap_or("");
                out.push('\n');
                out.push_str(&diff_lines(old, new));
            }
            Some(out)
        }
        "Write" => {
            let path = input.get("file_path")?.as_str()?;
            let content = input.get("content").and_then(|v| v.as_str()).unwrap_or("");
            let lines = content.lines().count();
            Some(format!("{BOLD}[write: {path}]{RESET} {DIM}({lines} lines){RESET}"))
        }
        "str_replace" | "write_file" => {
            let path = input
                .get("path")
                .or_else(|| input.get("file_path"))
                .and_then(|v| v.as_str())?;
            if tool_name == "write_file" {
                let content = input.get("content").and_then(|v| v.as_str()).unwrap_or("");
                let lines = content.lines().count();
                Some(format!("{BOLD}[write: {path}]{RESET} {DIM}({lines} lines){RESET}"))
            } else {
                let old = input
                    .get("old_str")
                    .or_else(|| input.get("old_string"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let new = input
                    .get("new_str")
                    .or_else(|| input.get("new_string"))
                    .and_then(|v| v.as_str())?;
                Some(format_edit_diff(path, old, new))
            }
        }
        "read_file" => {
            let path = input.get("path").and_then(|v| v.as_str())?;
            Some(format!("{DIM}[read: {path}]{RESET}"))
        }
        "bash" | "Bash" => {
            let cmd = input
                .get("command")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let preview: String = cmd.chars().take(80).collect();
            Some(format!("{DIM}[bash: {preview}]{RESET}"))
        }
        _ => None,
    }
}

fn format_edit_diff(path: &str, old: &str, new: &str) -> String {
    let mut out = format!("{DIM}--- {path}{RESET}\n{DIM}+++ {path}{RESET}\n");
    out.push_str(&diff_lines(old, new));
    out
}

fn diff_lines(old: &str, new: &str) -> String {
    let mut out = String::new();
    for line in old.lines() {
        out.push_str(&format!("{RED}-{line}{RESET}\n"));
    }
    for line in new.lines() {
        out.push_str(&format!("{GREEN}+{line}{RESET}\n"));
    }
    out
}

// ── Mock (test only) ──────────────────────────────────────────────────────────

#[cfg(test)]
pub mod mock {
    use super::*;
    use std::collections::VecDeque;
    use std::sync::Mutex;

    /// A mock `Session` for unit tests.
    ///
    /// Pre-load a sequence of event batches with `queue_turn`. Each `prompt` call
    /// drains one batch; if none are queued, a `Done("end_turn")` is returned.
    pub struct MockSession {
        session_id: String,
        turns: Mutex<VecDeque<Vec<StreamEvent>>>,
        cancelled: Mutex<Vec<String>>,
        closed: Mutex<u32>,
        compacted: Mutex<u32>,
        model: Mutex<Option<String>>,
        set_model_error: Mutex<Option<String>>,
        compact_error: Mutex<Option<String>>,
        last_cwd: Mutex<Option<PathBuf>>,
    }

    impl MockSession {
        pub fn new(session_id: impl Into<String>) -> Self {
            Self {
                session_id: session_id.into(),
                turns: Mutex::new(VecDeque::new()),
                cancelled: Mutex::new(Vec::new()),
                closed: Mutex::new(0),
                compacted: Mutex::new(0),
                model: Mutex::new(None),
                set_model_error: Mutex::new(None),
                compact_error: Mutex::new(None),
                last_cwd: Mutex::new(None),
            }
        }

        pub fn queue_turn(&self, events: Vec<StreamEvent>) {
            self.turns.lock().unwrap().push_back(events);
        }

        pub fn cancel_count(&self) -> usize {
            self.cancelled.lock().unwrap().len()
        }

        pub fn close_count(&self) -> u32 {
            *self.closed.lock().unwrap()
        }

        pub fn compact_count(&self) -> u32 {
            *self.compacted.lock().unwrap()
        }

        pub fn last_model(&self) -> Option<String> {
            self.model.lock().unwrap().clone()
        }

        pub fn fail_set_model(&self, error: impl Into<String>) {
            *self.set_model_error.lock().unwrap() = Some(error.into());
        }

        pub fn fail_compact(&self, error: impl Into<String>) {
            *self.compact_error.lock().unwrap() = Some(error.into());
        }

        pub fn last_cwd(&self) -> Option<PathBuf> {
            self.last_cwd.lock().unwrap().clone()
        }
    }

    impl Session for MockSession {
        fn session_id(&self) -> &str {
            &self.session_id
        }

        fn current_model(&self) -> String {
            self.model
                .lock()
                .unwrap()
                .clone()
                .unwrap_or_else(|| "claude-sonnet-4-6".into())
        }

        fn prompt(
            &self,
            _text: &str,
        ) -> impl std::future::Future<Output = anyhow::Result<mpsc::Receiver<StreamEvent>>> + Send + '_
        {
            let events = self
                .turns
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or_else(|| vec![StreamEvent::Done("end_turn".into())]);
            async move {
                let (tx, rx) = mpsc::channel(events.len().max(1));
                for event in events {
                    let _ = tx.try_send(event);
                }
                Ok(rx)
            }
        }

        fn cancel(&self) -> impl std::future::Future<Output = ()> + Send + '_ {
            let id = self.session_id.clone();
            async move {
                self.cancelled.lock().unwrap().push(id);
            }
        }

        fn set_model(
            &self,
            model_id: &str,
        ) -> impl std::future::Future<Output = anyhow::Result<()>> + Send + '_
        {
            let model_id = model_id.to_string();
            async move {
                if let Some(err) = self.set_model_error.lock().unwrap().clone() {
                    return Err(anyhow::anyhow!("{err}"));
                }
                *self.model.lock().unwrap() = Some(model_id);
                Ok(())
            }
        }

        fn compact(&self) -> impl std::future::Future<Output = anyhow::Result<CompactResult>> + Send + '_ {
            async move {
                if let Some(err) = self.compact_error.lock().unwrap().clone() {
                    return Err(anyhow::anyhow!("{err}"));
                }
                *self.compacted.lock().unwrap() += 1;
                Ok(CompactResult {
                    compacted: true,
                    tokens_before: 100,
                    tokens_after: 50,
                })
            }
        }

        fn load_session(
            &self,
            _session_id: &str,
            cwd: &Path,
            _mcp_servers: Vec<McpServer>,
        ) -> impl std::future::Future<Output = anyhow::Result<()>> + Send + '_ {
            let cwd = cwd.to_path_buf();
            async move {
                *self.last_cwd.lock().unwrap() = Some(cwd);
                Ok(())
            }
        }

        fn set_cwd(
            &self,
            cwd: &Path,
        ) -> impl std::future::Future<Output = anyhow::Result<()>> + Send + '_ {
            self.load_session(self.session_id(), cwd, vec![])
        }

        fn list_sessions(
            &self,
        ) -> impl std::future::Future<Output = anyhow::Result<Vec<SessionSummary>>> + Send + '_ {
            async move { Ok(vec![]) }
        }

        fn session_cwd(
            &self,
        ) -> impl std::future::Future<Output = anyhow::Result<Option<PathBuf>>> + Send + '_ {
            let cwd = self.last_cwd.lock().unwrap().clone();
            async move { Ok(cwd) }
        }

        fn set_mode(
            &self,
            _mode: &str,
        ) -> impl std::future::Future<Output = anyhow::Result<()>> + Send + '_
        {
            async move { Ok(()) }
        }

        fn close(&self) -> impl std::future::Future<Output = ()> + Send + '_ {
            async move {
                *self.closed.lock().unwrap() += 1;
            }
        }
    }

    impl Session for std::sync::Arc<MockSession> {
        fn session_id(&self) -> &str {
            (**self).session_id()
        }

        fn current_model(&self) -> String {
            (**self).current_model()
        }

        fn prompt(
            &self,
            text: &str,
        ) -> impl std::future::Future<Output = anyhow::Result<mpsc::Receiver<StreamEvent>>> + Send + '_
        {
            (**self).prompt(text)
        }

        fn cancel(&self) -> impl std::future::Future<Output = ()> + Send + '_ {
            (**self).cancel()
        }

        fn set_model(
            &self,
            model_id: &str,
        ) -> impl std::future::Future<Output = anyhow::Result<()>> + Send + '_
        {
            (**self).set_model(model_id)
        }

        fn compact(&self) -> impl std::future::Future<Output = anyhow::Result<CompactResult>> + Send + '_ {
            (**self).compact()
        }

        fn load_session(
            &self,
            session_id: &str,
            cwd: &Path,
            mcp_servers: Vec<McpServer>,
        ) -> impl std::future::Future<Output = anyhow::Result<()>> + Send + '_ {
            (**self).load_session(session_id, cwd, mcp_servers)
        }

        fn set_cwd(
            &self,
            cwd: &Path,
        ) -> impl std::future::Future<Output = anyhow::Result<()>> + Send + '_ {
            (**self).set_cwd(cwd)
        }

        fn list_sessions(
            &self,
        ) -> impl std::future::Future<Output = anyhow::Result<Vec<SessionSummary>>> + Send + '_ {
            (**self).list_sessions()
        }

        fn session_cwd(
            &self,
        ) -> impl std::future::Future<Output = anyhow::Result<Option<PathBuf>>> + Send + '_ {
            (**self).session_cwd()
        }

        fn set_mode(
            &self,
            mode: &str,
        ) -> impl std::future::Future<Output = anyhow::Result<()>> + Send + '_
        {
            (**self).set_mode(mode)
        }

        fn close(&self) -> impl std::future::Future<Output = ()> + Send + '_ {
            (**self).close()
        }
    }

    // ── MockSessionFactory ────────────────────────────────────────────────────

    pub struct MockSessionFactory {
        sessions: std::sync::Mutex<std::collections::VecDeque<std::sync::Arc<MockSession>>>,
        default_id: String,
    }

    impl MockSessionFactory {
        pub fn new(default_id: impl Into<String>) -> Self {
            Self {
                sessions: Default::default(),
                default_id: default_id.into(),
            }
        }

        pub fn push_session(&self, session: std::sync::Arc<MockSession>) {
            self.sessions.lock().unwrap().push_back(session);
        }
    }

    impl super::SessionFactory for MockSessionFactory {
        type Sess = std::sync::Arc<MockSession>;

        fn create_session<'a>(
            &'a self,
            _prefix: &'a str,
            _cwd: PathBuf,
            _mcp_servers: Vec<McpServer>,
        ) -> impl std::future::Future<Output = anyhow::Result<std::sync::Arc<MockSession>>> + 'a {
            async move {
                let session = self.sessions.lock().unwrap().pop_front()
                    .unwrap_or_else(|| std::sync::Arc::new(MockSession::new(&self.default_id)));
                Ok(session)
            }
        }

        fn attach_session(&self, _prefix: &str, session_id: String) -> std::sync::Arc<MockSession> {
            std::sync::Arc::new(MockSession::new(session_id))
        }
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nats::mock::MockNatsClient;
    use bytes::Bytes;
    use serde_json::json;

    /// Queue NATS mocks required by `TrogonSession::new` (session.new + default set_mode).
    async fn queue_new_session_setup(nats: &MockNatsClient, session_id: &str) {
        let resp = json!({"sessionId": session_id});
        nats.queue_request_ok(Bytes::from(serde_json::to_vec(&resp).unwrap()));
        let (tx, rx) = tokio::sync::mpsc::channel(1);
        nats.add_subscription(rx);
        tokio::spawn(async move {
            let _ = tx.send(Bytes::from_static(b"{}")).await;
        });
    }

    // ── render_diff ───────────────────────────────────────────────────────────

    #[test]
    fn render_diff_edit_returns_colored_diff() {
        let input = json!({"file_path": "src/main.rs", "old_string": "hello", "new_string": "world"});
        let diff = render_diff("Edit", Some(&input)).unwrap();
        assert!(diff.contains("src/main.rs"), "path in header");
        assert!(diff.contains("-hello"), "old line with minus");
        assert!(diff.contains("+world"), "new line with plus");
    }

    #[test]
    fn render_diff_edit_empty_old_string() {
        let input = json!({"file_path": "new.rs", "old_string": "", "new_string": "fn main() {}"});
        let diff = render_diff("Edit", Some(&input)).unwrap();
        assert!(diff.contains("+fn main() {}"));
        assert!(!diff.contains(&format!("{RED}-")), "no removal lines for empty old_string");
    }

    #[test]
    fn render_diff_multiedit_all_edits_shown() {
        let input = json!({
            "file_path": "lib.rs",
            "edits": [
                {"old_string": "foo", "new_string": "bar"},
                {"old_string": "baz", "new_string": "qux"},
            ]
        });
        let diff = render_diff("MultiEdit", Some(&input)).unwrap();
        assert!(diff.contains("-foo") && diff.contains("+bar"));
        assert!(diff.contains("-baz") && diff.contains("+qux"));
    }

    #[test]
    fn render_diff_write_shows_path_and_line_count() {
        let content = "line1\nline2\nline3";
        let input = json!({"file_path": "out.txt", "content": content});
        let diff = render_diff("Write", Some(&input)).unwrap();
        assert!(diff.contains("out.txt"));
        assert!(diff.contains("3 lines"));
    }

    #[test]
    fn render_diff_unknown_tool_returns_none() {
        let input = json!({"file_path": "x.rs"});
        assert!(render_diff("unknown_tool", Some(&input)).is_none());
    }

    #[test]
    fn render_diff_str_replace_returns_colored_diff() {
        let input = json!({
            "path": "src/main.rs",
            "old_str": "fn old()",
            "new_str": "fn new()"
        });
        let diff = render_diff("str_replace", Some(&input)).unwrap();
        assert!(diff.contains("src/main.rs"));
        assert!(diff.contains("fn old()"));
        assert!(diff.contains("fn new()"));
    }

    #[test]
    fn render_diff_bash_shows_command_preview() {
        let input = json!({"command": "cargo test -p trogon-cli"});
        let diff = render_diff("bash", Some(&input)).unwrap();
        assert!(diff.contains("cargo test"));
    }

    #[test]
    fn render_diff_read_file_shows_path() {
        let input = json!({"path": "Cargo.toml"});
        let diff = render_diff("read_file", Some(&input)).unwrap();
        assert!(diff.contains("Cargo.toml"));
    }

    #[test]
    fn render_diff_none_input_returns_none() {
        assert!(render_diff("Edit", None).is_none());
    }

    // ── wire-format compatibility ─────────────────────────────────────────────

    #[test]
    fn usage_update_deserializes_from_nats_wire_format() {
        let json = json!({
            "sessionId": "sess-1",
            "update": {
                "sessionUpdate": "usage_update",
                "used": 12345,
                "size": 200000
            }
        });
        let notif: agent_client_protocol::SessionNotification =
            serde_json::from_value(json).expect("must deserialize");
        match notif.update {
            agent_client_protocol::SessionUpdate::UsageUpdate(u) => {
                assert_eq!(u.used, 12345);
                assert_eq!(u.size, 200000);
            }
            other => panic!("expected UsageUpdate, got {other:?}"),
        }
    }

    #[test]
    fn tool_call_raw_input_is_accessible() {
        let json = json!({
            "sessionId": "sess-1",
            "update": {
                "sessionUpdate": "tool_call",
                "toolCallId": "tc-1",
                "title": "Edit",
                "rawInput": {
                    "file_path": "src/lib.rs",
                    "old_string": "foo",
                    "new_string": "bar"
                }
            }
        });
        let notif: agent_client_protocol::SessionNotification =
            serde_json::from_value(json).expect("must deserialize");
        match notif.update {
            agent_client_protocol::SessionUpdate::ToolCall(tc) => {
                assert_eq!(tc.title, "Edit");
                let input = tc.raw_input.expect("raw_input must be present");
                assert_eq!(input["file_path"].as_str().unwrap(), "src/lib.rs");
            }
            other => panic!("expected ToolCall, got {other:?}"),
        }
    }

    // ── TrogonSession::new via MockNatsClient ─────────────────────────────────

    #[test]
    fn parse_current_model_id_reads_runner_response() {
        let resp = json!({"sessionId": "s", "models": {"currentModelId": "grok-4"}});
        assert_eq!(parse_current_model_id(&resp, "acp.grok"), "grok-4");
    }

    #[test]
    fn parse_current_model_id_uses_prefix_default_when_missing() {
        let resp = json!({"sessionId": "s"});
        assert_eq!(
            parse_current_model_id(&resp, "acp.grok"),
            default_model_for_prefix("acp.grok")
        );
    }

    // ── TrogonSession::set_model ──────────────────────────────────────────────

    #[tokio::test]
    async fn set_model_sends_nats_request_and_returns_ok() {
        let nats = MockNatsClient::new();
        queue_new_session_setup(&nats, "s1").await;
        let session =
            TrogonSession::new(nats.clone(), "acp", std::path::PathBuf::from("/tmp"), vec![]).await.unwrap();

        let (tx, rx) = tokio::sync::mpsc::channel(1);
        nats.add_subscription(rx);
        tx.send(Bytes::from(b"{}".as_ref())).await.unwrap();
        let result = session.set_model("claude-opus-4-7").await;
        assert!(result.is_ok(), "expected Ok, got: {result:?}");
        assert_eq!(session.current_model(), "claude-opus-4-7");
    }

    #[tokio::test]
    async fn set_model_returns_error_on_nats_failure() {
        let nats = MockNatsClient::new();
        queue_new_session_setup(&nats, "s1").await;
        let session =
            TrogonSession::new(nats.clone(), "acp", std::path::PathBuf::from("/tmp"), vec![]).await.unwrap();

        // no subscription queued — subscribe_bytes will fail
        let err = session.set_model("claude-opus-4-7").await.unwrap_err();
        assert!(err.to_string().contains("NATS error"), "got: {err}");
    }

    // ── MockSession::set_model ────────────────────────────────────────────────

    #[tokio::test]
    async fn mock_session_set_model_stores_model() {
        use mock::MockSession;
        let session = MockSession::new("s");
        session.set_model("claude-opus-4-7").await.unwrap();
        assert_eq!(session.last_model().as_deref(), Some("claude-opus-4-7"));
    }

    #[tokio::test]
    async fn mock_session_set_model_returns_error_when_configured() {
        use mock::MockSession;
        let session = MockSession::new("s");
        session.fail_set_model("runner unavailable");
        let err = session.set_model("claude-opus-4-7").await.unwrap_err();
        assert!(err.to_string().contains("runner unavailable"), "got: {err}");
    }

    // ── TrogonSession::new via MockNatsClient ─────────────────────────────────

    #[tokio::test]
    async fn new_session_extracts_session_id_from_response() {
        let nats = MockNatsClient::new();
        queue_new_session_setup(&nats, "test-session-42").await;

        let session =
            TrogonSession::new(nats, "acp", std::path::PathBuf::from("/tmp"), vec![]).await.unwrap();
        assert_eq!(session.session_id(), "test-session-42");
    }

    #[tokio::test]
    async fn new_session_returns_error_on_nats_failure() {
        let nats = MockNatsClient::new();
        nats.queue_request_err("connection refused");

        let err = TrogonSession::new(nats, "acp", std::path::PathBuf::from("/tmp"), vec![])
            .await
            .unwrap_err();
        assert!(err.to_string().contains("NATS error"), "got: {err}");
    }

    #[tokio::test]
    async fn new_session_returns_error_on_missing_session_id() {
        let nats = MockNatsClient::new();
        let resp = json!({"other": "field"});
        nats.queue_request_ok(Bytes::from(serde_json::to_vec(&resp).unwrap()));

        let err = TrogonSession::new(nats, "acp", std::path::PathBuf::from("/tmp"), vec![])
            .await
            .unwrap_err();
        assert!(err.to_string().contains("sessionId"), "got: {err}");
    }

    // ── TrogonSession::prompt via MockNatsClient ──────────────────────────────

    #[tokio::test]
    async fn prompt_streams_text_events_and_done() {
        let nats = MockNatsClient::new();
        queue_new_session_setup(&nats, "s1").await;

        // Two subscriptions: notif channel + inbox (reply) channel.
        let (notif_tx, notif_rx) = tokio::sync::mpsc::channel::<Bytes>(8);
        let (reply_tx, reply_rx) = tokio::sync::mpsc::channel::<Bytes>(8);
        nats.add_subscription(notif_rx);
        nats.add_subscription(reply_rx);

        let session =
            TrogonSession::new(nats, "acp", std::path::PathBuf::from("/tmp"), vec![]).await.unwrap();

        let mut events_rx = session.prompt("hello").await.unwrap();

        // Send a text notification.
        let text_notif = json!({
            "sessionId": "s1",
            "update": {
                "sessionUpdate": "agent_message_chunk",
                "content": {"type": "text", "text": "hello world"}
            }
        });
        notif_tx.send(Bytes::from(serde_json::to_vec(&text_notif).unwrap())).await.unwrap();

        // Yield so the spawned task processes the notification before we send Done.
        // Without this, the biased select! would pick the Done reply first if both
        // channels are ready simultaneously.
        tokio::task::yield_now().await;

        // Send done reply.
        let done = json!({"stopReason": "end_turn"});
        reply_tx.send(Bytes::from(serde_json::to_vec(&done).unwrap())).await.unwrap();

        let mut got_text = false;
        let mut got_done = false;
        while let Some(ev) = events_rx.recv().await {
            match ev {
                StreamEvent::Text(t) => { assert_eq!(t, "hello world"); got_text = true; }
                StreamEvent::Done(r) => { assert_eq!(r, "end_turn"); got_done = true; break; }
                _ => {}
            }
        }
        assert!(got_text, "expected Text event");
        assert!(got_done, "expected Done event");
    }

    // ── MockSession ───────────────────────────────────────────────────────────

    #[tokio::test]
    async fn mock_session_returns_queued_events() {
        use mock::MockSession;
        let session = MockSession::new("mock-session");
        session.queue_turn(vec![
            StreamEvent::Text("hi".into()),
            StreamEvent::Done("end_turn".into()),
        ]);

        let mut rx = session.prompt("anything").await.unwrap();
        let ev1 = rx.recv().await.unwrap();
        let ev2 = rx.recv().await.unwrap();
        assert!(matches!(ev1, StreamEvent::Text(t) if t == "hi"));
        assert!(matches!(ev2, StreamEvent::Done(r) if r == "end_turn"));
    }

    #[tokio::test]
    async fn mock_session_default_turn_is_done() {
        use mock::MockSession;
        let session = MockSession::new("s");
        let mut rx = session.prompt("anything").await.unwrap();
        let ev = rx.recv().await.unwrap();
        assert!(matches!(ev, StreamEvent::Done(_)));
    }

    #[tokio::test]
    async fn mock_session_set_cwd_updates_last_cwd() {
        use mock::MockSession;
        let session = MockSession::new("s");
        session
            .set_cwd(std::path::Path::new("/new/project"))
            .await
            .unwrap();
        assert_eq!(
            session.last_cwd().as_deref(),
            Some(std::path::Path::new("/new/project"))
        );
    }

    // ── MockSession::compact ──────────────────────────────────────────────────

    #[tokio::test]
    async fn mock_session_compact_increments_count() {
        use mock::MockSession;
        let session = MockSession::new("s");
        session.compact().await.unwrap();
        session.compact().await.unwrap();
        assert_eq!(session.compact_count(), 2);
    }

    #[tokio::test]
    async fn mock_session_compact_returns_error_when_configured() {
        use mock::MockSession;
        let session = MockSession::new("s");
        session.fail_compact("compactor unavailable");
        let err = session.compact().await.unwrap_err();
        assert!(err.to_string().contains("compactor unavailable"), "got: {err}");
    }

    // ── TrogonSession::compact via MockNatsClient ─────────────────────────────

    fn ext_response(body: &str) -> Bytes {
        let resp = ExtResponse::new(
            serde_json::value::RawValue::from_string(body.to_string())
                .unwrap()
                .into(),
        );
        Bytes::from(serde_json::to_vec(&resp).unwrap())
    }

    #[tokio::test]
    async fn compact_export_compactor_import_round_trip() {
        let nats = MockNatsClient::new();
        queue_new_session_setup(&nats, "s1").await;
        let session =
            TrogonSession::new(nats.clone(), "acp", std::path::PathBuf::from("/tmp"), vec![]).await.unwrap();

        nats.queue_request_ok(ext_response(
            r#"[{"role":"user","text":"hello"},{"role":"assistant","text":"world"}]"#,
        ));
        nats.queue_request_ok(Bytes::from(
            serde_json::to_vec(&json!({
                "messages": [{"role": "user", "content": [{"type": "text", "text": "summary"}]}],
                "compacted": true,
                "tokens_before": 1000,
                "tokens_after": 200,
            }))
            .unwrap(),
        ));
        nats.queue_request_ok(ext_response("{}"));

        let result = session.compact().await.unwrap();
        assert_eq!(
            result,
            CompactResult {
                compacted: true,
                tokens_before: 1000,
                tokens_after: 200,
            }
        );
    }

    #[tokio::test]
    async fn compact_returns_noop_when_export_empty() {
        let nats = MockNatsClient::new();
        queue_new_session_setup(&nats, "s1").await;
        let session =
            TrogonSession::new(nats.clone(), "acp", std::path::PathBuf::from("/tmp"), vec![]).await.unwrap();

        nats.queue_request_ok(ext_response("[]"));

        let result = session.compact().await.unwrap();
        assert_eq!(
            result,
            CompactResult {
                compacted: false,
                tokens_before: 0,
                tokens_after: 0,
            }
        );
    }

    // ── NatsSessionFactory ────────────────────────────────────────────────────

    #[tokio::test]
    async fn nats_factory_create_session_returns_session_with_correct_id() {
        let nats = MockNatsClient::new();
        queue_new_session_setup(&nats, "factory-created-session").await;

        let factory = NatsSessionFactory::new(nats);
        let session = factory
            .create_session("acp", std::path::PathBuf::from("/tmp"), vec![])
            .await
            .unwrap();
        assert_eq!(session.session_id(), "factory-created-session");
    }

    #[tokio::test]
    async fn nats_factory_attach_session_returns_session_with_given_id() {
        let nats = MockNatsClient::new();
        let factory = NatsSessionFactory::new(nats);
        let session = factory.attach_session("acp", "pre-existing-id".to_string());
        assert_eq!(session.session_id(), "pre-existing-id");
    }

    // ── MockSessionFactory ────────────────────────────────────────────────────

    #[tokio::test]
    async fn mock_factory_create_returns_default_session_when_empty() {
        use mock::MockSessionFactory;
        let factory = MockSessionFactory::new("default-sess");
        let session = factory
            .create_session("acp", std::path::PathBuf::from("/tmp"), vec![])
            .await
            .unwrap();
        assert_eq!(session.session_id(), "default-sess");
    }

    #[tokio::test]
    async fn mock_factory_create_pops_queued_sessions_in_order() {
        use mock::{MockSession, MockSessionFactory};
        use std::sync::Arc;
        let factory = MockSessionFactory::new("fallback");
        factory.push_session(Arc::new(MockSession::new("first")));
        factory.push_session(Arc::new(MockSession::new("second")));

        let s1 = factory.create_session("acp", std::path::PathBuf::from("/tmp"), vec![]).await.unwrap();
        let s2 = factory.create_session("acp", std::path::PathBuf::from("/tmp"), vec![]).await.unwrap();
        let s3 = factory.create_session("acp", std::path::PathBuf::from("/tmp"), vec![]).await.unwrap();
        assert_eq!(s1.session_id(), "first");
        assert_eq!(s2.session_id(), "second");
        assert_eq!(s3.session_id(), "fallback");
    }

    #[tokio::test]
    async fn mock_factory_attach_creates_session_with_given_id() {
        use mock::MockSessionFactory;
        let factory = MockSessionFactory::new("default");
        let session = factory.attach_session("acp", "attached-id".to_string());
        assert_eq!(session.session_id(), "attached-id");
    }
}

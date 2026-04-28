//! HTTP API for interactive agent chat sessions.
//!
//! ## Routes
//!
//! | Method | Path                              | Action                        |
//! |--------|-----------------------------------|-------------------------------|
//! | GET    | `/sessions`                       | List tenant's sessions        |
//! | POST   | `/sessions`                       | Create a new session          |
//! | GET    | `/sessions/:id`                   | Get session with history      |
//! | PATCH  | `/sessions/:id`                   | Update name / model / tools   |
//! | DELETE | `/sessions/:id`                   | Delete session                |
//! | POST   | `/sessions/:id/messages`          | Send message, run agent       |
//!
//! ## Tenant identification
//! Every request must carry the `X-Tenant-Id` header.

use std::sync::Arc;

use axum::{
    Router,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tracing::{info, warn};

use crate::agent_loop::{AgentLoop, Message};
use crate::handlers::{DEFAULT_MEMORY_PATH, fetch_memory};
use crate::promise_store::{AgentPromise, PromiseRepository, PromiseStatus};
use crate::session::{ChatSession, SessionRepository};
use crate::tools::all_tool_defs;

// ── App state ─────────────────────────────────────────────────────────────────

#[derive(Clone)]
pub struct ChatAppState<R: SessionRepository> {
    pub agent: Arc<AgentLoop>,
    pub session_store: R,
    pub promise_store: Arc<dyn PromiseRepository>,
    /// Agent definition ID from CONSOLE_AGENTS (populated when `AGENT_ID` is set).
    pub agent_id: Option<String>,
    /// Reads current skill_ids for this agent from CONSOLE_AGENTS on each request
    /// so that updates to the agent definition propagate without a restart.
    pub agent_loader: Option<Arc<dyn crate::agent_loader::AgentLoading>>,
    /// Skill loader shared with the automation dispatcher.
    pub skill_loader: Option<Arc<dyn crate::skill_loader::SkillLoading>>,
    /// When `Some`, context compaction is attempted before each `run_chat` call
    /// via a NATS request to `trogon.compactor.compact`. Degrades gracefully.
    pub compactor_nats: Option<async_nats::Client>,
}

// ── Context compaction ────────────────────────────────────────────────────────

/// Sends the conversation history to `trogon-compactor` via NATS request-reply.
/// Returns the original messages unchanged if the compactor is unavailable.
#[cfg_attr(coverage, coverage(off))]
async fn compact_messages(
    nats: &async_nats::Client,
    messages: Vec<Message>,
    session_id: &str,
) -> Vec<Message> {
    #[derive(serde::Serialize)]
    struct CompactReq<'a> {
        messages: &'a [Message],
    }
    #[derive(serde::Deserialize)]
    struct CompactResp {
        messages: Vec<Message>,
        #[serde(default)]
        compacted: bool,
        #[serde(default)]
        tokens_before: usize,
        #[serde(default)]
        tokens_after: usize,
    }

    let Ok(payload) = serde_json::to_vec(&CompactReq { messages: &messages }) else {
        return messages;
    };
    let reply = match nats
        .request("trogon.compactor.compact", payload.into())
        .await
    {
        Ok(r) => r,
        Err(e) => {
            warn!(session_id, error = %e, "compactor unavailable — skipping compaction");
            return messages;
        }
    };
    match serde_json::from_slice::<CompactResp>(&reply.payload) {
        Ok(resp) => {
            if resp.compacted {
                info!(
                    session_id,
                    tokens_before = resp.tokens_before,
                    tokens_after = resp.tokens_after,
                    "context compacted"
                );
            }
            resp.messages
        }
        Err(e) => {
            warn!(session_id, error = %e, "compactor returned invalid response — skipping");
            messages
        }
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

#[allow(clippy::result_large_err)]
fn tenant_id(headers: &HeaderMap) -> Result<String, Response> {
    match headers.get("x-tenant-id").and_then(|v| v.to_str().ok()) {
        Some(t) if !t.is_empty() => Ok(t.to_string()),
        _ => Err(err(StatusCode::BAD_REQUEST, "missing X-Tenant-Id header")),
    }
}

fn err(status: StatusCode, msg: impl std::fmt::Display) -> Response {
    (status, axum::Json(json!({"error": msg.to_string()}))).into_response()
}

fn now_epoch_secs() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn now_iso8601() -> String {
    epoch_to_iso8601(now_epoch_secs())
}

pub(crate) fn epoch_to_iso8601(secs: u64) -> String {
    let s = secs % 60;
    let m = (secs / 60) % 60;
    let h = (secs / 3600) % 24;
    let days = secs / 86400;
    let mut year = 1970u64;
    let mut remaining = days;
    loop {
        let dy =
            if (year.is_multiple_of(4) && !year.is_multiple_of(100)) || year.is_multiple_of(400) {
                366
            } else {
                365
            };
        if remaining < dy {
            break;
        }
        remaining -= dy;
        year += 1;
    }
    let leap = (year.is_multiple_of(4) && !year.is_multiple_of(100)) || year.is_multiple_of(400);
    let months = [
        31u64,
        if leap { 29 } else { 28 },
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ];
    let mut month = 1u64;
    for &dim in &months {
        if remaining < dim {
            break;
        }
        remaining -= dim;
        month += 1;
    }
    format!(
        "{year:04}-{month:02}-{:02}T{h:02}:{m:02}:{s:02}Z",
        remaining + 1
    )
}

// ── Response types ─────────────────────────────────────────────────────────────

/// Session summary returned in list and create/update responses.
/// Does NOT include the full message history to keep list responses small.
#[derive(Debug, Serialize)]
struct SessionSummary {
    id: String,
    tenant_id: String,
    name: String,
    model: Option<String>,
    tools: Vec<String>,
    memory_path: Option<String>,
    message_count: usize,
    created_at: String,
    updated_at: String,
}

impl From<&ChatSession> for SessionSummary {
    fn from(s: &ChatSession) -> Self {
        Self {
            id: s.id.clone(),
            tenant_id: s.tenant_id.clone(),
            name: s.name.clone(),
            model: s.model.clone(),
            tools: s.tools.clone(),
            memory_path: s.memory_path.clone(),
            message_count: s.messages.len(),
            created_at: s.created_at.clone(),
            updated_at: s.updated_at.clone(),
        }
    }
}

// ── Request types ──────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct CreateSessionRequest {
    pub name: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub tools: Vec<String>,
    #[serde(default)]
    pub memory_path: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateSessionRequest {
    pub name: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub tools: Option<Vec<String>>,
    #[serde(default)]
    pub memory_path: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct SendMessageRequest {
    /// The user's message text.
    pub content: String,
}

#[derive(Debug, Serialize)]
pub struct SendMessageResponse {
    /// The assistant's final text response.
    pub content: String,
    /// Total messages in the session after this turn.
    pub message_count: usize,
}

// ── Handlers ──────────────────────────────────────────────────────────────────

async fn list_sessions<R: SessionRepository>(
    headers: HeaderMap,
    State(state): State<ChatAppState<R>>,
) -> Response {
    let tid = match tenant_id(&headers) {
        Ok(t) => t,
        Err(r) => return r,
    };
    match state.session_store.list(&tid).await {
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e),
        Ok(sessions) => {
            let summaries: Vec<SessionSummary> = sessions.iter().map(Into::into).collect();
            (StatusCode::OK, axum::Json(summaries)).into_response()
        }
    }
}

async fn create_session<R: SessionRepository>(
    headers: HeaderMap,
    State(state): State<ChatAppState<R>>,
    axum::Json(body): axum::Json<CreateSessionRequest>,
) -> Response {
    let tid = match tenant_id(&headers) {
        Ok(t) => t,
        Err(r) => return r,
    };
    let now_secs = now_epoch_secs();
    let now = epoch_to_iso8601(now_secs);
    let session = ChatSession {
        id: uuid::Uuid::new_v4().to_string(),
        tenant_id: tid,
        name: body.name.unwrap_or_else(|| "New Agent".to_string()),
        model: body.model,
        tools: body.tools,
        memory_path: body.memory_path,
        messages: vec![],
        created_at: now.clone(),
        updated_at: now,
        started_at_secs: now_secs,
        duration_ms: 0,
        agent_id: state.agent_id.clone(),
    };
    match state.session_store.put(&session).await {
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e),
        Ok(()) => (
            StatusCode::CREATED,
            axum::Json(SessionSummary::from(&session)),
        )
            .into_response(),
    }
}

async fn get_session<R: SessionRepository>(
    headers: HeaderMap,
    State(state): State<ChatAppState<R>>,
    Path(id): Path<String>,
) -> Response {
    let tid = match tenant_id(&headers) {
        Ok(t) => t,
        Err(r) => return r,
    };
    match state.session_store.get(&tid, &id).await {
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e),
        Ok(None) => err(StatusCode::NOT_FOUND, format!("session {id} not found")),
        // Return full session including messages for the detail view.
        Ok(Some(s)) => (StatusCode::OK, axum::Json(s)).into_response(),
    }
}

async fn update_session<R: SessionRepository>(
    headers: HeaderMap,
    State(state): State<ChatAppState<R>>,
    Path(id): Path<String>,
    axum::Json(body): axum::Json<UpdateSessionRequest>,
) -> Response {
    let tid = match tenant_id(&headers) {
        Ok(t) => t,
        Err(r) => return r,
    };
    let mut session = match state.session_store.get(&tid, &id).await {
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, e),
        Ok(None) => return err(StatusCode::NOT_FOUND, format!("session {id} not found")),
        Ok(Some(s)) => s,
    };
    if let Some(name) = body.name {
        session.name = name;
    }
    if let Some(model) = body.model {
        session.model = Some(model);
    }
    if let Some(tools) = body.tools {
        session.tools = tools;
    }
    if let Some(mp) = body.memory_path {
        session.memory_path = Some(mp);
    }
    session.updated_at = now_iso8601();
    match state.session_store.put(&session).await {
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e),
        Ok(()) => (StatusCode::OK, axum::Json(SessionSummary::from(&session))).into_response(),
    }
}

async fn delete_session<R: SessionRepository>(
    headers: HeaderMap,
    State(state): State<ChatAppState<R>>,
    Path(id): Path<String>,
) -> Response {
    let tid = match tenant_id(&headers) {
        Ok(t) => t,
        Err(r) => return r,
    };
    match state.session_store.get(&tid, &id).await {
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, e),
        Ok(None) => return err(StatusCode::NOT_FOUND, format!("session {id} not found")),
        Ok(Some(_)) => {}
    }
    match state.session_store.delete(&tid, &id).await {
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e),
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
    }
}

async fn send_message<R: SessionRepository>(
    headers: HeaderMap,
    State(state): State<ChatAppState<R>>,
    Path(id): Path<String>,
    axum::Json(body): axum::Json<SendMessageRequest>,
) -> Response {
    let tid = match tenant_id(&headers) {
        Ok(t) => t,
        Err(r) => return r,
    };

    let mut session = match state.session_store.get(&tid, &id).await {
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, e),
        Ok(None) => return err(StatusCode::NOT_FOUND, format!("session {id} not found")),
        Ok(Some(s)) => s,
    };

    // Append the new user message to the existing history.
    session.messages.push(Message::user_text(&body.content));

    // Auto-name the session from the first user message (truncated to 60 chars).
    if session.name == "New Agent" && session.messages.len() == 1 {
        session.name = body.content.chars().take(60).collect();
    }

    // Build the tool list — filtered by session.tools if non-empty.
    let all = all_tool_defs();
    let tools: Vec<_> = if session.tools.is_empty() {
        all
    } else {
        all.into_iter()
            .filter(|t| session.tools.contains(&t.name))
            .collect()
    };

    // Resolve the effective agent: override model if the session specifies one.
    let effective_model = session
        .model
        .clone()
        .unwrap_or_else(|| state.agent.model.clone());
    let temp_agent;
    let agent: &AgentLoop = if effective_model == state.agent.model {
        &state.agent
    } else {
        temp_agent = AgentLoop {
            anthropic_client: Arc::clone(&state.agent.anthropic_client),
            model: effective_model,
            max_iterations: state.agent.max_iterations,
            tool_dispatcher: Arc::clone(&state.agent.tool_dispatcher),
            tool_context: Arc::clone(&state.agent.tool_context),
            memory_owner: state.agent.memory_owner.clone(),
            memory_repo: state.agent.memory_repo.clone(),
            memory_path: state.agent.memory_path.clone(),
            mcp_tool_defs: state.agent.mcp_tool_defs.clone(),
            mcp_dispatch: state.agent.mcp_dispatch.clone(),
            flag_client: Arc::clone(&state.agent.flag_client),
            tenant_id: state.agent.tenant_id.clone(),
            promise_store: None,
            promise_id: None,
        };
        &temp_agent
    };

    // Fetch memory (system prompt).
    let mem_path = session
        .memory_path
        .as_deref()
        .or(agent.memory_path.as_deref())
        .unwrap_or(DEFAULT_MEMORY_PATH);
    let memory = match (&agent.memory_owner, &agent.memory_repo) {
        (Some(owner), Some(repo)) => fetch_memory(agent, owner, repo, mem_path).await,
        _ => None,
    };

    // Fetch current skill_ids from the agent definition (reads CONSOLE_AGENTS KV
    // on every turn so console updates propagate without an agent restart).
    let skill_ids: Vec<String> = match (&state.agent_loader, &state.agent_id) {
        (Some(loader), Some(agent_id)) => loader.get_skill_ids(agent_id).await,
        _ => vec![],
    };

    // Load skills for this agent definition and combine with memory.
    let skill_content: Option<String> = match &state.skill_loader {
        Some(loader) if !skill_ids.is_empty() => loader.load(&skill_ids).await,
        _ => None,
    };
    let system_prompt: Option<String> = match (skill_content.as_deref(), memory.as_deref()) {
        (Some(skills), Some(mem)) => Some(format!("{skills}\n\n---\n\n{mem}")),
        (Some(skills), None) => Some(skills.to_string()),
        (None, Some(mem)) => Some(mem.to_string()),
        (None, None) => None,
    };

    // Compact context window before sending to the LLM (degrades gracefully).
    if let Some(ref nats) = state.compactor_nats {
        let msgs = session.messages.clone();
        session.messages = compact_messages(nats, msgs, &id).await;
    }

    // Run the chat loop — returns (final_text, updated_messages).
    match agent
        .run_chat(session.messages.clone(), &tools, system_prompt.as_deref())
        .await
    {
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
        Ok((text, updated_messages)) => {
            let message_count = updated_messages.len();
            session.messages = updated_messages;
            let now_secs = now_epoch_secs();
            session.updated_at = epoch_to_iso8601(now_secs);
            // Only compute duration for sessions that have a valid start time.
            // Legacy sessions with started_at_secs == 0 are skipped to avoid
            // reporting a nonsensical ~55-year duration.
            if session.started_at_secs > 0 {
                session.duration_ms = now_secs
                    .saturating_sub(session.started_at_secs)
                    .saturating_mul(1000);
            }

            if let Err(e) = state.session_store.put(&session).await {
                return err(StatusCode::INTERNAL_SERVER_ERROR, e);
            }

            (
                StatusCode::OK,
                axum::Json(SendMessageResponse {
                    content: text,
                    message_count,
                }),
            )
                .into_response()
        }
    }
}

// ── Router ────────────────────────────────────────────────────────────────────

// ── Promise admin ─────────────────────────────────────────────────────────────

/// Summary view of a running promise returned by `GET /admin/promises`.
///
/// Excludes `messages` and `system_prompt` — both can be very large and are
/// not useful for operational inspection.
#[derive(Serialize)]
struct PromiseView {
    id: String,
    automation_id: String,
    status: PromiseStatus,
    iteration: u32,
    worker_id: String,
    claimed_at: u64,
    nats_subject: String,
    recovery_count: u32,
    checkpoint_degraded: bool,
    failure_reason: Option<String>,
}

impl From<AgentPromise> for PromiseView {
    fn from(p: AgentPromise) -> Self {
        Self {
            id: p.id,
            automation_id: p.automation_id,
            status: p.status,
            iteration: p.iteration,
            worker_id: p.worker_id,
            claimed_at: p.claimed_at,
            nats_subject: p.nats_subject,
            recovery_count: p.recovery_count,
            checkpoint_degraded: p.checkpoint_degraded,
            failure_reason: p.failure_reason,
        }
    }
}

/// `GET /admin/promises` — list all Running promises for the tenant.
///
/// Useful for operators to identify stuck promises without direct NATS CLI
/// access. Only returns Running promises; terminal ones (Resolved,
/// PermanentFailed) are cleaned up by KV TTL and not returned.
async fn list_promises<R: SessionRepository>(
    State(state): State<ChatAppState<R>>,
    headers: HeaderMap,
) -> Response {
    let tenant_id = match tenant_id(&headers) {
        Ok(t) => t,
        Err(r) => return r,
    };
    match tokio::time::timeout(
        std::time::Duration::from_secs(10),
        state.promise_store.list_running(&tenant_id),
    )
    .await
    {
        Ok(Ok(promises)) => {
            let views: Vec<PromiseView> = promises.into_iter().map(PromiseView::from).collect();
            axum::Json(views).into_response()
        }
        Ok(Err(e)) => err(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("promise store error: {e}"),
        ),
        Err(_) => err(StatusCode::GATEWAY_TIMEOUT, "promise store timed out"),
    }
}

pub fn router<R: SessionRepository>(state: ChatAppState<R>) -> Router {
    Router::new()
        .route(
            "/sessions",
            get(list_sessions::<R>).post(create_session::<R>),
        )
        .route(
            "/sessions/{id}",
            get(get_session::<R>)
                .patch(update_session::<R>)
                .delete(delete_session::<R>),
        )
        .route("/sessions/{id}/messages", post(send_message::<R>))
        .route("/admin/promises", get(list_promises::<R>))
        .with_state(state)
}

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::mock::{ErrorSessionStore, GetOkPutErrorSessionStore, MockSessionStore};
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use tower::util::ServiceExt as _;

    #[test]
    fn now_iso8601_has_correct_format() {
        let ts = now_iso8601();
        assert_eq!(ts.len(), 20);
        assert!(ts.ends_with('Z'));
    }

    #[test]
    fn session_summary_message_count() {
        let s = ChatSession {
            id: "s1".to_string(),
            tenant_id: "t".to_string(),
            name: "n".to_string(),
            model: None,
            tools: vec![],
            memory_path: None,
            messages: vec![Message::user_text("hi"), Message::user_text("hey")],
            created_at: "2026-01-01T00:00:00Z".to_string(),
            updated_at: "2026-01-01T00:00:00Z".to_string(),
            started_at_secs: 0,
            duration_ms: 0,
            agent_id: None,
        };
        let summary = SessionSummary::from(&s);
        assert_eq!(summary.message_count, 2);
    }

    #[test]
    fn tenant_id_missing_returns_err() {
        let headers = HeaderMap::new();
        assert!(tenant_id(&headers).is_err());
    }

    #[test]
    fn tenant_id_valid_returns_ok() {
        let mut headers = HeaderMap::new();
        headers.insert("x-tenant-id", "acme".parse().unwrap());
        assert_eq!(tenant_id(&headers).unwrap(), "acme");
    }

    #[test]
    fn tenant_id_empty_string_returns_err() {
        // HTTP header values can contain empty strings; treat them as missing.
        let mut headers = HeaderMap::new();
        headers.insert("x-tenant-id", "".parse().unwrap());
        assert!(
            tenant_id(&headers).is_err(),
            "empty X-Tenant-Id must be rejected"
        );
    }

    // ── epoch_to_iso8601 ──────────────────────────────────────────────────────

    /// Epoch 0 → 1970-01-01T00:00:00Z
    #[test]
    fn epoch_to_iso8601_epoch_zero_is_unix_epoch() {
        assert_eq!(epoch_to_iso8601(0), "1970-01-01T00:00:00Z");
    }

    /// Feb 29 in a leap year (2024-02-29T00:00:00Z = epoch 1709164800).
    #[test]
    fn epoch_to_iso8601_leap_year_feb_29() {
        // 2024 is a leap year (divisible by 4, not by 100).
        // 2024-02-29T00:00:00Z in Unix time:
        //   2024-01-01T00:00:00Z = 1704067200
        //   + 31 days (Jan) + 28 days (to reach Feb 29) = 59 days = 5097600 s
        let epoch = 1704067200u64 + 59 * 86400;
        assert_eq!(epoch_to_iso8601(epoch), "2024-02-29T00:00:00Z");
    }

    /// Feb 28 in a non-leap year — no 29th day should appear.
    #[test]
    fn epoch_to_iso8601_non_leap_year_feb_28() {
        // 2023 is not a leap year.
        // 2023-02-28T00:00:00Z = 1677542400
        //   2023-01-01T00:00:00Z = 1672531200 + 31 days (Jan) + 27 days = 58 days
        let epoch = 1672531200u64 + 58 * 86400;
        assert_eq!(epoch_to_iso8601(epoch), "2023-02-28T00:00:00Z");
    }

    /// Year 2000 is a leap year (divisible by 400) — Mar 1 must follow Feb 29.
    #[test]
    fn epoch_to_iso8601_year_2000_is_leap_year() {
        // 2000-02-29T00:00:00Z
        // 2000-01-01T00:00:00Z = 946684800
        // +31 (Jan) +28 (to Feb 29) = 59 days
        let epoch = 946684800u64 + 59 * 86400;
        assert_eq!(epoch_to_iso8601(epoch), "2000-02-29T00:00:00Z");
    }

    /// Year 1900 is not a leap year (divisible by 100 but not 400) — but since
    /// we start from 1970 we verify 1996 (a regular quadrennial leap year).
    #[test]
    fn epoch_to_iso8601_1996_is_leap_year() {
        // 1996-02-29T00:00:00Z
        // 1996-01-01T00:00:00Z = 820454400
        // +31 (Jan) +28 (to Feb 29) = 59 days
        let epoch = 820454400u64 + 59 * 86400;
        assert_eq!(epoch_to_iso8601(epoch), "1996-02-29T00:00:00Z");
    }

    /// Time component — seconds, minutes, hours are correctly decomposed.
    #[test]
    fn epoch_to_iso8601_time_of_day() {
        // 1970-01-01T15:30:45Z = 15*3600 + 30*60 + 45 = 55845
        assert_eq!(epoch_to_iso8601(55845), "1970-01-01T15:30:45Z");
    }

    /// Dec 31 — last day of a non-leap year (2023-12-31).
    /// Verifies the December month boundary is handled correctly.
    #[test]
    fn epoch_to_iso8601_dec_31_non_leap_year() {
        // 2023-01-01T00:00:00Z = 1672531200
        // Jan(31)+Feb(28)+Mar(31)+Apr(30)+May(31)+Jun(30)+Jul(31)+Aug(31)+Sep(30)+Oct(31)+Nov(30)
        // = 334 days → day 334 is Dec 1 (0-indexed).  Dec 31 is day 364.
        let epoch = 1672531200u64 + 364 * 86400;
        assert_eq!(epoch_to_iso8601(epoch), "2023-12-31T00:00:00Z");
    }

    /// Mar 1 in a non-leap year (2023-03-01) — the day immediately after Feb 28.
    /// Verifies no off-by-one when transitioning out of February in a regular year.
    #[test]
    fn epoch_to_iso8601_mar_01_after_feb_28_non_leap() {
        // 2023-01-01T00:00:00Z = 1672531200
        // Jan(31) + Feb(28) = 59 days → 0-indexed day 59 is Mar 1.
        let epoch = 1672531200u64 + 59 * 86400;
        assert_eq!(epoch_to_iso8601(epoch), "2023-03-01T00:00:00Z");
    }

    /// Mar 1 in a leap year (2024-03-01) — the day immediately after Feb 29.
    /// Verifies no off-by-one when transitioning out of February in a leap year.
    #[test]
    fn epoch_to_iso8601_mar_01_after_feb_29_leap() {
        // 2024-01-01T00:00:00Z = 1704067200
        // Jan(31) + Feb(29, leap) = 60 days → 0-indexed day 60 is Mar 1.
        let epoch = 1704067200u64 + 60 * 86400;
        assert_eq!(epoch_to_iso8601(epoch), "2024-03-01T00:00:00Z");
    }

    /// Year 2100 is divisible by 4 and 100 but NOT 400 → NOT a leap year.
    /// Verifies the "divisible by 100 but not 400 → not leap" branch of the
    /// leap-year formula.  If 2100 were (incorrectly) treated as a leap year
    /// this epoch would map to "2100-02-29", not "2100-03-01".
    #[test]
    fn epoch_to_iso8601_year_2100_is_not_leap() {
        // Days from 1970-01-01 to 2100-01-01:
        //   130 years × 365 = 47450 days
        //   + 32 leap years in [1970, 2099]: 1972,1976,...,2096 (all ÷4, none ÷100 except
        //     2000 which is ÷400 and therefore IS leap) = 32 leap days
        //   = 47482 days  →  epoch 47482 × 86400 = 4102444800
        //
        // Jan(31) + Feb(28, NOT leap) = 59 days → Mar 1 is 0-indexed day 59.
        let epoch = 4102444800u64 + 59 * 86400;
        assert_eq!(
            epoch_to_iso8601(epoch),
            "2100-03-01T00:00:00Z",
            "2100 must NOT be treated as a leap year (÷100 but not ÷400)"
        );
    }

    /// Last second of a day (23:59:59) — verifies that the time decomposition
    /// does not bleed into the next day.
    #[test]
    fn epoch_to_iso8601_last_second_of_day() {
        // 1970-01-01T23:59:59Z = 86399
        assert_eq!(epoch_to_iso8601(86399), "1970-01-01T23:59:59Z");
    }

    // ── Backward-compat deserialization ──────────────────────────────────────

    /// A `ChatSession` serialized without the `model` field (as stored by an
    /// older version of the service) must deserialize successfully with
    /// `model = None`.  This validates the serde `default` attribute on the
    /// field keeps old records readable after the field was added.
    #[test]
    fn chat_session_without_model_field_deserializes_to_none() {
        use crate::session::ChatSession;

        let json = r#"{
            "id": "sess-1",
            "tenant_id": "acme",
            "name": "old-session",
            "tools": [],
            "messages": [],
            "created_at": "2025-01-01T00:00:00Z",
            "updated_at": "2025-01-01T00:00:00Z"
        }"#;

        let session: ChatSession = serde_json::from_str(json)
            .expect("ChatSession must deserialize even when 'model' field is absent");

        assert_eq!(session.id, "sess-1");
        assert!(
            session.model.is_none(),
            "model must be None when absent from the stored JSON"
        );
    }

    // ── Handler tests ─────────────────────────────────────────────────────────

    fn make_test_agent() -> Arc<AgentLoop> {
        use crate::agent_loop::ReqwestAnthropicClient;
        use crate::flag_client::AlwaysOnFlagClient;
        use crate::tools::{DefaultToolDispatcher, ToolContext};

        let http = reqwest::Client::new();
        let tool_ctx = Arc::new(ToolContext::for_test("http://127.0.0.1:1", "", "", ""));
        Arc::new(AgentLoop {
            anthropic_client: Arc::new(ReqwestAnthropicClient::new(
                http,
                "http://127.0.0.1:1".to_string(),
                String::new(),
            )),
            model: "test".to_string(),
            max_iterations: 1,
            tool_dispatcher: Arc::new(DefaultToolDispatcher::new(Arc::clone(&tool_ctx))),
            tool_context: tool_ctx,
            memory_owner: None,
            memory_repo: None,
            memory_path: None,
            mcp_tool_defs: vec![],
            mcp_dispatch: vec![],
            flag_client: Arc::new(AlwaysOnFlagClient),
            tenant_id: "test-tenant".to_string(),
            promise_store: None,
            promise_id: None,
        })
    }

    fn get_ok_put_error_app() -> axum::Router {
        let state = ChatAppState {
            agent: make_test_agent(),
            session_store: GetOkPutErrorSessionStore::new(),
            promise_store: Arc::new(crate::promise_store::mock::MockPromiseStore::new()),
            agent_id: None,
            agent_loader: None,
            skill_loader: None,
            compactor_nats: None,
        };
        router(state)
    }

    fn error_app() -> axum::Router {
        let state = ChatAppState {
            agent: make_test_agent(),
            session_store: ErrorSessionStore::new(),
            promise_store: Arc::new(crate::promise_store::mock::MockPromiseStore::new()),
            agent_id: None,
            agent_loader: None,
            skill_loader: None,
            compactor_nats: None,
        };
        router(state)
    }

    fn mock_app(store: MockSessionStore) -> axum::Router {
        let state = ChatAppState {
            agent: make_test_agent(),
            session_store: store,
            promise_store: Arc::new(crate::promise_store::mock::MockPromiseStore::new()),
            agent_id: None,
            agent_loader: None,
            skill_loader: None,
            compactor_nats: None,
        };
        router(state)
    }

    fn get_req(path: &str, tenant: &str) -> Request<Body> {
        Request::builder()
            .method("GET")
            .uri(path)
            .header("x-tenant-id", tenant)
            .body(Body::empty())
            .unwrap()
    }

    fn sample_session(id: &str, tenant: &str) -> ChatSession {
        ChatSession {
            id: id.to_string(),
            tenant_id: tenant.to_string(),
            name: "Test".to_string(),
            model: None,
            tools: vec![],
            memory_path: None,
            messages: vec![],
            created_at: "2026-01-01T00:00:00Z".to_string(),
            updated_at: "2026-01-02T00:00:00Z".to_string(),
            started_at_secs: 0,
            duration_ms: 0,
            agent_id: None,
        }
    }

    // list_sessions

    #[tokio::test]
    async fn list_sessions_returns_empty_when_no_sessions() {
        let app = mock_app(MockSessionStore::new());
        let resp = app.oneshot(get_req("/sessions", "acme")).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json, serde_json::json!([]));
    }

    #[tokio::test]
    async fn list_sessions_returns_sessions_for_tenant() {
        let store = MockSessionStore::new();
        store.insert(sample_session("s1", "acme"));
        store.insert(sample_session("s2", "acme"));
        store.insert(sample_session("s3", "other-tenant"));
        let app = mock_app(store);
        let resp = app.oneshot(get_req("/sessions", "acme")).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json.as_array().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn list_sessions_missing_tenant_returns_400() {
        let app = mock_app(MockSessionStore::new());
        let req = Request::builder()
            .method("GET")
            .uri("/sessions")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn create_session_missing_tenant_returns_400() {
        let app = mock_app(MockSessionStore::new());
        let req = Request::builder()
            .method("POST")
            .uri("/sessions")
            .header("content-type", "application/json")
            .body(Body::from(r#"{"name":"My Session"}"#))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    // create_session

    #[tokio::test]
    async fn create_session_returns_201_and_persists() {
        let store = MockSessionStore::new();
        let app = mock_app(store.clone());
        let req = Request::builder()
            .method("POST")
            .uri("/sessions")
            .header("x-tenant-id", "acme")
            .header("content-type", "application/json")
            .body(Body::from(r#"{"name":"My Session"}"#))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["name"], "My Session");
        assert_eq!(json["tenant_id"], "acme");
        assert_eq!(store.snapshot().len(), 1);
    }

    #[tokio::test]
    async fn create_session_uses_default_name_when_omitted() {
        let app = mock_app(MockSessionStore::new());
        let req = Request::builder()
            .method("POST")
            .uri("/sessions")
            .header("x-tenant-id", "acme")
            .header("content-type", "application/json")
            .body(Body::from(r#"{}"#))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["name"], "New Agent");
    }

    #[tokio::test]
    async fn get_session_missing_tenant_returns_400() {
        let app = mock_app(MockSessionStore::new());
        let req = Request::builder()
            .method("GET")
            .uri("/sessions/sess-1")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    // get_session

    #[tokio::test]
    async fn get_session_returns_404_when_not_found() {
        let app = mock_app(MockSessionStore::new());
        let resp = app
            .oneshot(get_req("/sessions/does-not-exist", "acme"))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn get_session_returns_session_with_messages() {
        let store = MockSessionStore::new();
        let mut s = sample_session("sess-1", "acme");
        s.messages = vec![Message::user_text("hi")];
        store.insert(s);
        let app = mock_app(store);
        let resp = app
            .oneshot(get_req("/sessions/sess-1", "acme"))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["id"], "sess-1");
        assert_eq!(json["messages"].as_array().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn update_session_missing_tenant_returns_400() {
        let app = mock_app(MockSessionStore::new());
        let req = Request::builder()
            .method("PATCH")
            .uri("/sessions/sess-1")
            .header("content-type", "application/json")
            .body(Body::from(r#"{"name":"New Name"}"#))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    // update_session

    #[tokio::test]
    async fn update_session_returns_404_when_not_found() {
        let app = mock_app(MockSessionStore::new());
        let req = Request::builder()
            .method("PATCH")
            .uri("/sessions/no-such-session")
            .header("x-tenant-id", "acme")
            .header("content-type", "application/json")
            .body(Body::from(r#"{"name":"New Name"}"#))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn update_session_updates_model_field() {
        let store = MockSessionStore::new();
        store.insert(sample_session("sess-2", "acme"));
        let app = mock_app(store.clone());
        let req = Request::builder()
            .method("PATCH")
            .uri("/sessions/sess-2")
            .header("x-tenant-id", "acme")
            .header("content-type", "application/json")
            .body(Body::from(r#"{"model":"claude-opus-4-6"}"#))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["model"], "claude-opus-4-6");
        assert_eq!(
            store.snapshot()["acme.sess-2"].model.as_deref(),
            Some("claude-opus-4-6")
        );
    }

    #[tokio::test]
    async fn update_session_updates_tools_field() {
        let store = MockSessionStore::new();
        store.insert(sample_session("sess-3", "acme"));
        let app = mock_app(store.clone());
        let req = Request::builder()
            .method("PATCH")
            .uri("/sessions/sess-3")
            .header("x-tenant-id", "acme")
            .header("content-type", "application/json")
            .body(Body::from(
                r#"{"tools":["post_pr_comment","send_slack_message"]}"#,
            ))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let snap = store.snapshot();
        assert_eq!(
            snap["acme.sess-3"].tools,
            vec!["post_pr_comment", "send_slack_message"]
        );
    }

    #[tokio::test]
    async fn update_session_updates_memory_path_field() {
        let store = MockSessionStore::new();
        store.insert(sample_session("sess-4", "acme"));
        let app = mock_app(store.clone());
        let req = Request::builder()
            .method("PATCH")
            .uri("/sessions/sess-4")
            .header("x-tenant-id", "acme")
            .header("content-type", "application/json")
            .body(Body::from(r#"{"memory_path":".trogon/memory.md"}"#))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let snap = store.snapshot();
        assert_eq!(
            snap["acme.sess-4"].memory_path.as_deref(),
            Some(".trogon/memory.md")
        );
    }

    #[tokio::test]
    async fn update_session_updates_multiple_fields_at_once() {
        let store = MockSessionStore::new();
        store.insert(sample_session("sess-5", "acme"));
        let app = mock_app(store.clone());
        let req = Request::builder()
            .method("PATCH")
            .uri("/sessions/sess-5")
            .header("x-tenant-id", "acme")
            .header("content-type", "application/json")
            .body(Body::from(
                r#"{"name":"Multi","model":"claude-haiku-4-5","tools":["get_pr_diff"]}"#,
            ))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let snap = store.snapshot();
        let s = &snap["acme.sess-5"];
        assert_eq!(s.name, "Multi");
        assert_eq!(s.model.as_deref(), Some("claude-haiku-4-5"));
        assert_eq!(s.tools, vec!["get_pr_diff"]);
    }

    #[tokio::test]
    async fn update_session_updates_name_and_returns_summary() {
        let store = MockSessionStore::new();
        store.insert(sample_session("sess-1", "acme"));
        let app = mock_app(store.clone());
        let req = Request::builder()
            .method("PATCH")
            .uri("/sessions/sess-1")
            .header("x-tenant-id", "acme")
            .header("content-type", "application/json")
            .body(Body::from(r#"{"name":"Updated"}"#))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["name"], "Updated");
        let snap = store.snapshot();
        assert_eq!(snap["acme.sess-1"].name, "Updated");
    }

    // delete_session

    #[tokio::test]
    async fn delete_session_returns_404_when_not_found() {
        let app = mock_app(MockSessionStore::new());
        let req = Request::builder()
            .method("DELETE")
            .uri("/sessions/no-such")
            .header("x-tenant-id", "acme")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn delete_session_removes_session_and_returns_204() {
        let store = MockSessionStore::new();
        store.insert(sample_session("sess-1", "acme"));
        let app = mock_app(store.clone());
        let req = Request::builder()
            .method("DELETE")
            .uri("/sessions/sess-1")
            .header("x-tenant-id", "acme")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);
        assert!(store.snapshot().is_empty());
    }

    #[tokio::test]
    async fn delete_session_missing_tenant_returns_400() {
        let app = mock_app(MockSessionStore::new());
        let req = Request::builder()
            .method("DELETE")
            .uri("/sessions/sess-1")
            // no x-tenant-id header
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    // send_message

    #[tokio::test]
    async fn send_message_missing_tenant_returns_400() {
        let app = mock_app(MockSessionStore::new());
        let req = Request::builder()
            .method("POST")
            .uri("/sessions/sess-1/messages")
            // no x-tenant-id header
            .header("content-type", "application/json")
            .body(Body::from(r#"{"content":"hello"}"#))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    // list_promises

    fn mock_app_with_promise_store(
        session_store: MockSessionStore,
        promise_store: Arc<dyn crate::promise_store::PromiseRepository>,
    ) -> axum::Router {
        let state = ChatAppState {
            agent: make_test_agent(),
            session_store,
            promise_store,
            agent_id: None,
            agent_loader: None,
            skill_loader: None,
            compactor_nats: None,
        };
        router(state)
    }

    fn make_running_promise(id: &str, tenant: &str) -> crate::promise_store::AgentPromise {
        crate::promise_store::AgentPromise {
            id: id.to_string(),
            tenant_id: tenant.to_string(),
            automation_id: "auto-1".to_string(),
            status: crate::promise_store::PromiseStatus::Running,
            messages: vec![crate::agent_loop::Message::user_text("big message hidden")],
            iteration: 3,
            worker_id: "host-pid".to_string(),
            claimed_at: 1000,
            trigger: serde_json::json!({}),
            nats_subject: "github.pull_request".to_string(),
            system_prompt: Some("long prompt hidden".to_string()),
            recovery_count: 1,
            checkpoint_degraded: false,
            failure_reason: None,
        }
    }

    #[tokio::test]
    async fn list_promises_empty_returns_empty_array() {
        let ps = Arc::new(crate::promise_store::mock::MockPromiseStore::new());
        let app = mock_app_with_promise_store(MockSessionStore::new(), ps);
        let resp = app
            .oneshot(get_req("/admin/promises", "acme"))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json, serde_json::json!([]));
    }

    #[tokio::test]
    async fn list_promises_returns_running_promises_without_messages_or_system_prompt() {
        let ps = Arc::new(crate::promise_store::mock::MockPromiseStore::new());
        ps.insert_promise(make_running_promise("p1", "acme"));
        let app = mock_app_with_promise_store(MockSessionStore::new(), Arc::clone(&ps) as _);
        let resp = app
            .oneshot(get_req("/admin/promises", "acme"))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let arr = json.as_array().unwrap();
        assert_eq!(arr.len(), 1);
        let view = &arr[0];
        // Required fields are present.
        assert_eq!(view["id"], "p1");
        assert_eq!(view["automation_id"], "auto-1");
        assert_eq!(view["iteration"], 3);
        assert_eq!(view["recovery_count"], 1);
        assert_eq!(view["checkpoint_degraded"], false);
        // messages and system_prompt must NOT appear in the response.
        assert!(
            view.get("messages").is_none(),
            "messages must be excluded from view"
        );
        assert!(
            view.get("system_prompt").is_none(),
            "system_prompt must be excluded from view"
        );
    }

    #[tokio::test]
    async fn list_promises_missing_tenant_returns_400() {
        let ps = Arc::new(crate::promise_store::mock::MockPromiseStore::new());
        let app = mock_app_with_promise_store(MockSessionStore::new(), ps);
        let req = Request::builder()
            .method("GET")
            .uri("/admin/promises")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn list_promises_store_error_returns_500() {
        use crate::promise_store::mock::ErrorListRunningStore;
        let ps = Arc::new(ErrorListRunningStore::new());
        let app = mock_app_with_promise_store(
            MockSessionStore::new(),
            Arc::clone(&ps) as Arc<dyn crate::promise_store::PromiseRepository>,
        );
        let resp = app
            .oneshot(get_req("/admin/promises", "acme"))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }

    #[tokio::test(start_paused = true)]
    async fn list_promises_store_timeout_returns_504() {
        use crate::promise_store::mock::HangingListRunningStore;
        let ps = Arc::new(HangingListRunningStore::new());
        let app = mock_app_with_promise_store(
            MockSessionStore::new(),
            Arc::clone(&ps) as Arc<dyn crate::promise_store::PromiseRepository>,
        );
        let (resp, _) = tokio::join!(
            app.oneshot(get_req("/admin/promises", "acme")),
            tokio::time::advance(std::time::Duration::from_secs(11)),
        );
        assert_eq!(resp.unwrap().status(), StatusCode::GATEWAY_TIMEOUT);
    }

    // send_message

    #[tokio::test]
    async fn send_message_returns_404_when_session_not_found() {
        let app = mock_app(MockSessionStore::new());
        let req = Request::builder()
            .method("POST")
            .uri("/sessions/no-such/messages")
            .header("x-tenant-id", "acme")
            .header("content-type", "application/json")
            .body(Body::from(r#"{"content":"hello"}"#))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    // ── store-error → 500 ─────────────────────────────────────────────────────

    #[tokio::test]
    async fn list_sessions_store_error_returns_500() {
        let resp = error_app()
            .oneshot(get_req("/sessions", "acme"))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }

    #[tokio::test]
    async fn create_session_store_error_returns_500() {
        let req = Request::builder()
            .method("POST")
            .uri("/sessions")
            .header("x-tenant-id", "acme")
            .header("content-type", "application/json")
            .body(Body::from(r#"{"name":"S"}"#))
            .unwrap();
        let resp = error_app().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }

    #[tokio::test]
    async fn get_session_store_error_returns_500() {
        let resp = error_app()
            .oneshot(get_req("/sessions/any-id", "acme"))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }

    #[tokio::test]
    async fn update_session_store_error_returns_500() {
        let req = Request::builder()
            .method("PATCH")
            .uri("/sessions/any-id")
            .header("x-tenant-id", "acme")
            .header("content-type", "application/json")
            .body(Body::from(r#"{"name":"New"}"#))
            .unwrap();
        let resp = error_app().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }

    #[tokio::test]
    async fn delete_session_store_error_returns_500() {
        let req = Request::builder()
            .method("DELETE")
            .uri("/sessions/any-id")
            .header("x-tenant-id", "acme")
            .body(Body::empty())
            .unwrap();
        let resp = error_app().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }

    /// `update_session` first calls `get()` (succeeds) then `put()`. When `put()`
    /// fails the handler must return 500 — the `ErrorSessionStore` never reaches
    /// this path because its `get()` always fails first.
    #[tokio::test]
    async fn update_session_put_error_returns_500() {
        let req = Request::builder()
            .method("PATCH")
            .uri("/sessions/dummy")
            .header("x-tenant-id", "acme")
            .header("content-type", "application/json")
            .body(Body::from(r#"{"name":"New Name"}"#))
            .unwrap();
        let resp = get_ok_put_error_app().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }

    /// `delete_session` first calls `get()` (succeeds) then `delete()`. When
    /// `delete()` fails the handler must return 500.
    #[tokio::test]
    async fn delete_session_delete_error_returns_500() {
        let req = Request::builder()
            .method("DELETE")
            .uri("/sessions/dummy")
            .header("x-tenant-id", "acme")
            .body(Body::empty())
            .unwrap();
        let resp = get_ok_put_error_app().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }

    #[tokio::test]
    async fn send_message_store_error_returns_500() {
        let req = Request::builder()
            .method("POST")
            .uri("/sessions/any-id/messages")
            .header("x-tenant-id", "acme")
            .header("content-type", "application/json")
            .body(Body::from(r#"{"content":"hello"}"#))
            .unwrap();
        let resp = error_app().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }
}

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

use crate::agent_loop::{AgentLoop, Message};
use crate::handlers::{DEFAULT_MEMORY_PATH, fetch_memory};
use crate::session::{ChatSession, SessionStore};
use crate::tools::all_tool_defs;

// ── App state ─────────────────────────────────────────────────────────────────

#[derive(Clone)]
pub struct ChatAppState {
    pub agent: Arc<AgentLoop>,
    pub session_store: SessionStore,
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn tenant_id(headers: &HeaderMap) -> Result<String, Response> {
    match headers.get("x-tenant-id").and_then(|v| v.to_str().ok()) {
        Some(t) if !t.is_empty() => Ok(t.to_string()),
        _ => Err(err(StatusCode::BAD_REQUEST, "missing X-Tenant-Id header")),
    }
}

fn err(status: StatusCode, msg: impl std::fmt::Display) -> Response {
    (status, axum::Json(json!({"error": msg.to_string()}))).into_response()
}

fn now_iso8601() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    // reuse epoch_to_parts logic inline
    let s = secs % 60;
    let m = (secs / 60) % 60;
    let h = (secs / 3600) % 24;
    let days = secs / 86400;
    let mut year = 1970u64;
    let mut remaining = days;
    loop {
        let dy = if (year % 4 == 0 && year % 100 != 0) || year % 400 == 0 { 366 } else { 365 };
        if remaining < dy { break; }
        remaining -= dy;
        year += 1;
    }
    let leap = (year % 4 == 0 && year % 100 != 0) || year % 400 == 0;
    let months = [31u64, if leap { 29 } else { 28 }, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    let mut month = 1u64;
    for &dim in &months {
        if remaining < dim { break; }
        remaining -= dim;
        month += 1;
    }
    format!("{year:04}-{month:02}-{:02}T{h:02}:{m:02}:{s:02}Z", remaining + 1)
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

async fn list_sessions(
    headers: HeaderMap,
    State(state): State<ChatAppState>,
) -> Response {
    let tid = match tenant_id(&headers) { Ok(t) => t, Err(r) => return r };
    match state.session_store.list(&tid).await {
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e),
        Ok(sessions) => {
            let summaries: Vec<SessionSummary> = sessions.iter().map(Into::into).collect();
            (StatusCode::OK, axum::Json(summaries)).into_response()
        }
    }
}

async fn create_session(
    headers: HeaderMap,
    State(state): State<ChatAppState>,
    axum::Json(body): axum::Json<CreateSessionRequest>,
) -> Response {
    let tid = match tenant_id(&headers) { Ok(t) => t, Err(r) => return r };
    let now = now_iso8601();
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
    };
    match state.session_store.put(&session).await {
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e),
        Ok(()) => (StatusCode::CREATED, axum::Json(SessionSummary::from(&session))).into_response(),
    }
}

async fn get_session(
    headers: HeaderMap,
    State(state): State<ChatAppState>,
    Path(id): Path<String>,
) -> Response {
    let tid = match tenant_id(&headers) { Ok(t) => t, Err(r) => return r };
    match state.session_store.get(&tid, &id).await {
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e),
        Ok(None) => err(StatusCode::NOT_FOUND, format!("session {id} not found")),
        // Return full session including messages for the detail view.
        Ok(Some(s)) => (StatusCode::OK, axum::Json(s)).into_response(),
    }
}

async fn update_session(
    headers: HeaderMap,
    State(state): State<ChatAppState>,
    Path(id): Path<String>,
    axum::Json(body): axum::Json<UpdateSessionRequest>,
) -> Response {
    let tid = match tenant_id(&headers) { Ok(t) => t, Err(r) => return r };
    let mut session = match state.session_store.get(&tid, &id).await {
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, e),
        Ok(None) => return err(StatusCode::NOT_FOUND, format!("session {id} not found")),
        Ok(Some(s)) => s,
    };
    if let Some(name) = body.name { session.name = name; }
    if let Some(model) = body.model { session.model = Some(model); }
    if let Some(tools) = body.tools { session.tools = tools; }
    if let Some(mp) = body.memory_path { session.memory_path = Some(mp); }
    session.updated_at = now_iso8601();
    match state.session_store.put(&session).await {
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e),
        Ok(()) => (StatusCode::OK, axum::Json(SessionSummary::from(&session))).into_response(),
    }
}

async fn delete_session(
    headers: HeaderMap,
    State(state): State<ChatAppState>,
    Path(id): Path<String>,
) -> Response {
    let tid = match tenant_id(&headers) { Ok(t) => t, Err(r) => return r };
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

async fn send_message(
    headers: HeaderMap,
    State(state): State<ChatAppState>,
    Path(id): Path<String>,
    axum::Json(body): axum::Json<SendMessageRequest>,
) -> Response {
    let tid = match tenant_id(&headers) { Ok(t) => t, Err(r) => return r };

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
        all.into_iter().filter(|t| session.tools.contains(&t.name)).collect()
    };

    // Resolve the effective agent: override model if the session specifies one.
    let effective_model = session.model.clone().unwrap_or_else(|| state.agent.model.clone());
    let temp_agent;
    let agent: &AgentLoop = if effective_model == state.agent.model {
        &state.agent
    } else {
        temp_agent = AgentLoop {
            http_client: state.agent.http_client.clone(),
            proxy_url: state.agent.proxy_url.clone(),
            anthropic_token: state.agent.anthropic_token.clone(),
            anthropic_base_url: state.agent.anthropic_base_url.clone(),
            anthropic_extra_headers: state.agent.anthropic_extra_headers.clone(),
            model: effective_model,
            max_iterations: state.agent.max_iterations,
            thinking_budget: state.agent.thinking_budget,
            tool_context: Arc::clone(&state.agent.tool_context),
            memory_owner: state.agent.memory_owner.clone(),
            memory_repo: state.agent.memory_repo.clone(),
            memory_path: state.agent.memory_path.clone(),
            mcp_tool_defs: state.agent.mcp_tool_defs.clone(),
            mcp_dispatch: state.agent.mcp_dispatch.clone(),
            split_client: state.agent.split_client.clone(),
            tenant_id: state.agent.tenant_id.clone(),
            permission_checker: state.agent.permission_checker.clone(),
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

    // Run the chat loop — returns (final_text, updated_messages).
    match agent.run_chat(session.messages.clone(), &tools, memory.as_deref()).await {
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
        Ok((text, updated_messages)) => {
            let message_count = updated_messages.len();
            session.messages = updated_messages;
            session.updated_at = now_iso8601();

            if let Err(e) = state.session_store.put(&session).await {
                return err(StatusCode::INTERNAL_SERVER_ERROR, e);
            }

            (StatusCode::OK, axum::Json(SendMessageResponse { content: text, message_count }))
                .into_response()
        }
    }
}

// ── Router ────────────────────────────────────────────────────────────────────

pub fn router(state: ChatAppState) -> Router {
    Router::new()
        .route("/sessions", get(list_sessions).post(create_session))
        .route(
            "/sessions/{id}",
            get(get_session).patch(update_session).delete(delete_session),
        )
        .route("/sessions/{id}/messages", post(send_message))
        .with_state(state)
}

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

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
}

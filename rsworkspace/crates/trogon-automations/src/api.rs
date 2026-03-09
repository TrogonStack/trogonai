//! Axum HTTP API for managing automations.
//!
//! ## Tenant identification
//! Every request must carry the `X-Tenant-Id` header.  Missing or empty
//! values are rejected with `400 Bad Request`.
//!
//! ## Routes
//!
//! | Method | Path                          | Action              |
//! |--------|-------------------------------|---------------------|
//! | GET    | `/automations`                | List tenant's       |
//! | POST   | `/automations`                | Create              |
//! | GET    | `/automations/:id`            | Get one             |
//! | PUT    | `/automations/:id`            | Replace             |
//! | DELETE | `/automations/:id`            | Delete              |
//! | PATCH  | `/automations/:id/enable`     | Enable (no restart) |
//! | PATCH  | `/automations/:id/disable`    | Disable (no restart)|

use axum::{
    Router,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, patch},
};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::automation::{Automation, McpServer, Visibility};
use crate::runs::RunStore;
use crate::store::AutomationStore;

// ── App state ─────────────────────────────────────────────────────────────────

#[derive(Clone)]
pub struct AppState {
    pub store: AutomationStore,
    pub run_store: RunStore,
}

// ── Tenant extraction ─────────────────────────────────────────────────────────

fn tenant_id(headers: &HeaderMap) -> Result<String, Response> {
    match headers.get("x-tenant-id").and_then(|v| v.to_str().ok()) {
        Some(t) if !t.is_empty() => Ok(t.to_string()),
        _ => Err(err(StatusCode::BAD_REQUEST, "missing X-Tenant-Id header")),
    }
}

// ── Error helper ──────────────────────────────────────────────────────────────

fn err(status: StatusCode, msg: impl std::fmt::Display) -> Response {
    (status, axum::Json(json!({"error": msg.to_string()}))).into_response()
}

// ── Request bodies ────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct CreateRequest {
    pub name: String,
    pub trigger: String,
    pub prompt: String,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub tools: Vec<String>,
    pub memory_path: Option<String>,
    #[serde(default)]
    pub mcp_servers: Vec<McpServer>,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub visibility: Visibility,
}

#[derive(Debug, Deserialize)]
pub struct UpdateRequest {
    pub name: String,
    pub trigger: String,
    pub prompt: String,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub tools: Vec<String>,
    pub memory_path: Option<String>,
    #[serde(default)]
    pub mcp_servers: Vec<McpServer>,
    pub enabled: bool,
    #[serde(default)]
    pub visibility: Visibility,
}

fn default_true() -> bool {
    true
}

// ── Response body ─────────────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
struct AutomationResponse {
    id: String,
    tenant_id: String,
    name: String,
    trigger: String,
    prompt: String,
    model: Option<String>,
    tools: Vec<String>,
    memory_path: Option<String>,
    mcp_servers: Vec<McpServer>,
    enabled: bool,
    visibility: Visibility,
    created_at: String,
    updated_at: String,
}

impl From<Automation> for AutomationResponse {
    fn from(a: Automation) -> Self {
        Self {
            id: a.id,
            tenant_id: a.tenant_id,
            name: a.name,
            trigger: a.trigger,
            prompt: a.prompt,
            model: a.model,
            tools: a.tools,
            memory_path: a.memory_path,
            mcp_servers: a.mcp_servers,
            enabled: a.enabled,
            visibility: a.visibility,
            created_at: a.created_at,
            updated_at: a.updated_at,
        }
    }
}

// ── Timestamp helper ──────────────────────────────────────────────────────────

fn now_iso8601() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let (y, mo, d, h, mi, s) = epoch_to_parts(secs);
    format!("{y:04}-{mo:02}-{d:02}T{h:02}:{mi:02}:{s:02}Z")
}

fn epoch_to_parts(secs: u64) -> (u64, u64, u64, u64, u64, u64) {
    let s = secs % 60;
    let m = (secs / 60) % 60;
    let h = (secs / 3600) % 24;
    let days = secs / 86400;
    let mut year = 1970u64;
    let mut remaining = days;
    loop {
        let days_in_year = if is_leap(year) { 366 } else { 365 };
        if remaining < days_in_year { break; }
        remaining -= days_in_year;
        year += 1;
    }
    let months = [31u64, if is_leap(year) { 29 } else { 28 }, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    let mut month = 1u64;
    for &dim in &months {
        if remaining < dim { break; }
        remaining -= dim;
        month += 1;
    }
    (year, month, remaining + 1, h, m, s)
}

fn is_leap(y: u64) -> bool {
    (y % 4 == 0 && y % 100 != 0) || y % 400 == 0
}

// ── Handlers ──────────────────────────────────────────────────────────────────

async fn list_automations(
    headers: HeaderMap,
    State(state): State<AppState>,
) -> Response {
    let tid = match tenant_id(&headers) { Ok(t) => t, Err(r) => return r };
    match state.store.list(&tid).await {
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e),
        Ok(automations) => {
            let resp: Vec<AutomationResponse> = automations.into_iter().map(Into::into).collect();
            (StatusCode::OK, axum::Json(resp)).into_response()
        }
    }
}

async fn create_automation(
    headers: HeaderMap,
    State(state): State<AppState>,
    axum::Json(body): axum::Json<CreateRequest>,
) -> Response {
    let tid = match tenant_id(&headers) { Ok(t) => t, Err(r) => return r };
    let now = now_iso8601();
    let automation = Automation {
        id: uuid::Uuid::new_v4().to_string(),
        tenant_id: tid,
        name: body.name,
        trigger: body.trigger,
        prompt: body.prompt,
        model: body.model,
        tools: body.tools,
        memory_path: body.memory_path,
        mcp_servers: body.mcp_servers,
        enabled: body.enabled,
        visibility: body.visibility,
        created_at: now.clone(),
        updated_at: now,
    };
    match state.store.put(&automation).await {
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e),
        Ok(()) => (StatusCode::CREATED, axum::Json(AutomationResponse::from(automation))).into_response(),
    }
}

async fn get_automation(
    headers: HeaderMap,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Response {
    let tid = match tenant_id(&headers) { Ok(t) => t, Err(r) => return r };
    match state.store.get(&tid, &id).await {
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e),
        Ok(None) => err(StatusCode::NOT_FOUND, format!("automation {id} not found")),
        Ok(Some(a)) => (StatusCode::OK, axum::Json(AutomationResponse::from(a))).into_response(),
    }
}

async fn update_automation(
    headers: HeaderMap,
    State(state): State<AppState>,
    Path(id): Path<String>,
    axum::Json(body): axum::Json<UpdateRequest>,
) -> Response {
    let tid = match tenant_id(&headers) { Ok(t) => t, Err(r) => return r };
    let existing = match state.store.get(&tid, &id).await {
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, e),
        Ok(None) => return err(StatusCode::NOT_FOUND, format!("automation {id} not found")),
        Ok(Some(a)) => a,
    };
    let updated = Automation {
        id: existing.id,
        tenant_id: existing.tenant_id,
        name: body.name,
        trigger: body.trigger,
        prompt: body.prompt,
        model: body.model,
        tools: body.tools,
        memory_path: body.memory_path,
        mcp_servers: body.mcp_servers,
        enabled: body.enabled,
        visibility: body.visibility,
        created_at: existing.created_at,
        updated_at: now_iso8601(),
    };
    match state.store.put(&updated).await {
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e),
        Ok(()) => (StatusCode::OK, axum::Json(AutomationResponse::from(updated))).into_response(),
    }
}

async fn delete_automation(
    headers: HeaderMap,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Response {
    let tid = match tenant_id(&headers) { Ok(t) => t, Err(r) => return r };
    match state.store.get(&tid, &id).await {
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, e),
        Ok(None) => return err(StatusCode::NOT_FOUND, format!("automation {id} not found")),
        Ok(Some(_)) => {}
    }
    match state.store.delete(&tid, &id).await {
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e),
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
    }
}

async fn enable_automation(
    headers: HeaderMap,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Response {
    set_enabled(headers, state, id, true).await
}

async fn disable_automation(
    headers: HeaderMap,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Response {
    set_enabled(headers, state, id, false).await
}

async fn set_enabled(headers: HeaderMap, state: AppState, id: String, enabled: bool) -> Response {
    let tid = match tenant_id(&headers) { Ok(t) => t, Err(r) => return r };
    let mut automation = match state.store.get(&tid, &id).await {
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, e),
        Ok(None) => return err(StatusCode::NOT_FOUND, format!("automation {id} not found")),
        Ok(Some(a)) => a,
    };
    automation.enabled = enabled;
    automation.updated_at = now_iso8601();
    match state.store.put(&automation).await {
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e),
        Ok(()) => (StatusCode::OK, axum::Json(AutomationResponse::from(automation))).into_response(),
    }
}

// ── Run history handlers ───────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct RunsQuery {
    pub automation_id: Option<String>,
}

async fn list_runs(
    headers: HeaderMap,
    State(state): State<AppState>,
    Query(q): Query<RunsQuery>,
) -> Response {
    let tid = match tenant_id(&headers) { Ok(t) => t, Err(r) => return r };
    match state.run_store.list(&tid, q.automation_id.as_deref()).await {
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e),
        Ok(runs) => (StatusCode::OK, axum::Json(runs)).into_response(),
    }
}

async fn list_runs_for_automation(
    headers: HeaderMap,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Response {
    let tid = match tenant_id(&headers) { Ok(t) => t, Err(r) => return r };
    match state.run_store.list(&tid, Some(&id)).await {
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e),
        Ok(runs) => (StatusCode::OK, axum::Json(runs)).into_response(),
    }
}

async fn get_stats(
    headers: HeaderMap,
    State(state): State<AppState>,
) -> Response {
    let tid = match tenant_id(&headers) { Ok(t) => t, Err(r) => return r };
    match state.run_store.stats(&tid).await {
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e),
        Ok(stats) => (StatusCode::OK, axum::Json(stats)).into_response(),
    }
}

// ── Router ────────────────────────────────────────────────────────────────────

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/automations", get(list_automations).post(create_automation))
        .route(
            "/automations/{id}",
            get(get_automation).put(update_automation).delete(delete_automation),
        )
        .route("/automations/{id}/enable", patch(enable_automation))
        .route("/automations/{id}/disable", patch(disable_automation))
        .route("/automations/{id}/runs", get(list_runs_for_automation))
        .route("/runs", get(list_runs))
        .route("/stats", get(get_stats))
        .with_state(state)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tenant_id_missing_header_returns_err() {
        let headers = HeaderMap::new();
        assert!(tenant_id(&headers).is_err());
    }

    #[test]
    fn tenant_id_empty_header_returns_err() {
        let mut headers = HeaderMap::new();
        headers.insert("x-tenant-id", "".parse().unwrap());
        assert!(tenant_id(&headers).is_err());
    }

    #[test]
    fn tenant_id_valid_header_returns_ok() {
        let mut headers = HeaderMap::new();
        headers.insert("x-tenant-id", "acme".parse().unwrap());
        assert_eq!(tenant_id(&headers).unwrap(), "acme");
    }

    #[test]
    fn epoch_to_parts_epoch_zero() {
        let (y, mo, d, h, mi, s) = epoch_to_parts(0);
        assert_eq!((y, mo, d, h, mi, s), (1970, 1, 1, 0, 0, 0));
    }

    #[test]
    fn now_iso8601_has_correct_format() {
        let ts = now_iso8601();
        assert_eq!(ts.len(), 20, "unexpected length: {ts}");
        assert!(ts.ends_with('Z'));
    }

    #[test]
    fn is_leap_year() {
        assert!(is_leap(2000));
        assert!(is_leap(2024));
        assert!(!is_leap(1900));
        assert!(!is_leap(2023));
    }

    #[test]
    fn default_true_returns_true() {
        assert!(default_true());
    }
}

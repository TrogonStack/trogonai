//! Axum HTTP API for managing automations.
//!
//! Routes:
//!
//! | Method | Path                          | Action              |
//! |--------|-------------------------------|---------------------|
//! | GET    | `/automations`                | List all            |
//! | POST   | `/automations`                | Create              |
//! | GET    | `/automations/:id`            | Get one             |
//! | PUT    | `/automations/:id`            | Replace             |
//! | DELETE | `/automations/:id`            | Delete              |
//! | PATCH  | `/automations/:id/enable`     | Enable (no restart) |
//! | PATCH  | `/automations/:id/disable`    | Disable (no restart)|

use axum::{
    Router,
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, patch},
};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::automation::{Automation, McpServer};
use crate::store::AutomationStore;

// ── App state ─────────────────────────────────────────────────────────────────

#[derive(Clone)]
pub struct AppState {
    pub store: AutomationStore,
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
    pub tools: Vec<String>,
    pub memory_path: Option<String>,
    #[serde(default)]
    pub mcp_servers: Vec<McpServer>,
    #[serde(default = "default_true")]
    pub enabled: bool,
}

#[derive(Debug, Deserialize)]
pub struct UpdateRequest {
    pub name: String,
    pub trigger: String,
    pub prompt: String,
    #[serde(default)]
    pub tools: Vec<String>,
    pub memory_path: Option<String>,
    #[serde(default)]
    pub mcp_servers: Vec<McpServer>,
    pub enabled: bool,
}

fn default_true() -> bool {
    true
}

// ── Response body ─────────────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
struct AutomationResponse {
    id: String,
    name: String,
    trigger: String,
    prompt: String,
    tools: Vec<String>,
    memory_path: Option<String>,
    mcp_servers: Vec<McpServer>,
    enabled: bool,
    created_at: String,
    updated_at: String,
}

impl From<Automation> for AutomationResponse {
    fn from(a: Automation) -> Self {
        Self {
            id: a.id,
            name: a.name,
            trigger: a.trigger,
            prompt: a.prompt,
            tools: a.tools,
            memory_path: a.memory_path,
            mcp_servers: a.mcp_servers,
            enabled: a.enabled,
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
    // Minimal ISO-8601 UTC representation without extra deps.
    let (y, mo, d, h, mi, s) = epoch_to_parts(secs);
    format!("{y:04}-{mo:02}-{d:02}T{h:02}:{mi:02}:{s:02}Z")
}

/// Convert UNIX seconds to (year, month, day, hour, min, sec) UTC.
fn epoch_to_parts(secs: u64) -> (u64, u64, u64, u64, u64, u64) {
    let s = secs % 60;
    let m = (secs / 60) % 60;
    let h = (secs / 3600) % 24;
    let days = secs / 86400;

    // Gregorian calendar computation.
    let mut year = 1970u64;
    let mut remaining = days;
    loop {
        let days_in_year = if is_leap(year) { 366 } else { 365 };
        if remaining < days_in_year {
            break;
        }
        remaining -= days_in_year;
        year += 1;
    }
    let months = [31u64, if is_leap(year) { 29 } else { 28 }, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    let mut month = 1u64;
    for &days_in_month in &months {
        if remaining < days_in_month {
            break;
        }
        remaining -= days_in_month;
        month += 1;
    }
    (year, month, remaining + 1, h, m, s)
}

fn is_leap(y: u64) -> bool {
    (y % 4 == 0 && y % 100 != 0) || y % 400 == 0
}

// ── Handlers ──────────────────────────────────────────────────────────────────

async fn list_automations(State(state): State<AppState>) -> Response {
    match state.store.list().await {
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e),
        Ok(automations) => {
            let resp: Vec<AutomationResponse> = automations.into_iter().map(Into::into).collect();
            (StatusCode::OK, axum::Json(resp)).into_response()
        }
    }
}

async fn create_automation(
    State(state): State<AppState>,
    axum::Json(body): axum::Json<CreateRequest>,
) -> Response {
    let now = now_iso8601();
    let automation = Automation {
        id: uuid::Uuid::new_v4().to_string(),
        name: body.name,
        trigger: body.trigger,
        prompt: body.prompt,
        tools: body.tools,
        memory_path: body.memory_path,
        mcp_servers: body.mcp_servers,
        enabled: body.enabled,
        created_at: now.clone(),
        updated_at: now,
    };

    match state.store.put(&automation).await {
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e),
        Ok(()) => {
            let resp: AutomationResponse = automation.into();
            (StatusCode::CREATED, axum::Json(resp)).into_response()
        }
    }
}

async fn get_automation(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Response {
    match state.store.get(&id).await {
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e),
        Ok(None) => err(StatusCode::NOT_FOUND, format!("automation {id} not found")),
        Ok(Some(a)) => {
            let resp: AutomationResponse = a.into();
            (StatusCode::OK, axum::Json(resp)).into_response()
        }
    }
}

async fn update_automation(
    State(state): State<AppState>,
    Path(id): Path<String>,
    axum::Json(body): axum::Json<UpdateRequest>,
) -> Response {
    let existing = match state.store.get(&id).await {
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, e),
        Ok(None) => return err(StatusCode::NOT_FOUND, format!("automation {id} not found")),
        Ok(Some(a)) => a,
    };

    let updated = Automation {
        id: existing.id,
        name: body.name,
        trigger: body.trigger,
        prompt: body.prompt,
        tools: body.tools,
        memory_path: body.memory_path,
        mcp_servers: body.mcp_servers,
        enabled: body.enabled,
        created_at: existing.created_at,
        updated_at: now_iso8601(),
    };

    match state.store.put(&updated).await {
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e),
        Ok(()) => {
            let resp: AutomationResponse = updated.into();
            (StatusCode::OK, axum::Json(resp)).into_response()
        }
    }
}

async fn delete_automation(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Response {
    match state.store.get(&id).await {
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, e),
        Ok(None) => return err(StatusCode::NOT_FOUND, format!("automation {id} not found")),
        Ok(Some(_)) => {}
    }

    match state.store.delete(&id).await {
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e),
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
    }
}

async fn enable_automation(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Response {
    set_enabled(state, id, true).await
}

async fn disable_automation(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Response {
    set_enabled(state, id, false).await
}

async fn set_enabled(state: AppState, id: String, enabled: bool) -> Response {
    let mut automation = match state.store.get(&id).await {
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, e),
        Ok(None) => return err(StatusCode::NOT_FOUND, format!("automation {id} not found")),
        Ok(Some(a)) => a,
    };

    automation.enabled = enabled;
    automation.updated_at = now_iso8601();

    match state.store.put(&automation).await {
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e),
        Ok(()) => {
            let resp: AutomationResponse = automation.into();
            (StatusCode::OK, axum::Json(resp)).into_response()
        }
    }
}

// ── Router ────────────────────────────────────────────────────────────────────

/// Build the Axum router.  Bind it to a `TcpListener` in your `main`:
///
/// ```ignore
/// let listener = tokio::net::TcpListener::bind("0.0.0.0:8090").await?;
/// axum::serve(listener, router(state)).await?;
/// ```
pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/automations", get(list_automations).post(create_automation))
        .route(
            "/automations/:id",
            get(get_automation).put(update_automation).delete(delete_automation),
        )
        .route("/automations/:id/enable", patch(enable_automation))
        .route("/automations/:id/disable", patch(disable_automation))
        .with_state(state)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn epoch_to_parts_epoch_zero() {
        let (y, mo, d, h, mi, s) = epoch_to_parts(0);
        assert_eq!((y, mo, d, h, mi, s), (1970, 1, 1, 0, 0, 0));
    }

    #[test]
    fn now_iso8601_has_correct_format() {
        let ts = now_iso8601();
        // Must be "YYYY-MM-DDTHH:MM:SSZ" — 20 chars.
        assert_eq!(ts.len(), 20, "unexpected length: {ts}");
        assert!(ts.ends_with('Z'), "must end with Z: {ts}");
        assert_eq!(&ts[4..5], "-");
        assert_eq!(&ts[7..8], "-");
        assert_eq!(&ts[10..11], "T");
        assert_eq!(&ts[13..14], ":");
        assert_eq!(&ts[16..17], ":");
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

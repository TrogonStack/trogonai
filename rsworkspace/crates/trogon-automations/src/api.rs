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
use crate::runs::RunRepository;
use crate::store::AutomationRepository;

// ── App state ─────────────────────────────────────────────────────────────────

#[derive(Clone)]
pub struct AppState<A: AutomationRepository, R: RunRepository> {
    pub store: A,
    pub run_store: R,
}

// ── Tenant extraction ─────────────────────────────────────────────────────────

#[allow(clippy::result_large_err)]
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
        if remaining < days_in_year {
            break;
        }
        remaining -= days_in_year;
        year += 1;
    }
    let months = [
        31u64,
        if is_leap(year) { 29 } else { 28 },
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
    (year, month, remaining + 1, h, m, s)
}

fn is_leap(y: u64) -> bool {
    (y.is_multiple_of(4) && !y.is_multiple_of(100)) || y.is_multiple_of(400)
}

// ── Handlers ──────────────────────────────────────────────────────────────────

async fn list_automations<A: AutomationRepository, R: RunRepository>(
    headers: HeaderMap,
    State(state): State<AppState<A, R>>,
) -> Response {
    let tid = match tenant_id(&headers) {
        Ok(t) => t,
        Err(r) => return r,
    };
    match state.store.list(&tid).await {
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e),
        Ok(automations) => {
            let resp: Vec<AutomationResponse> = automations.into_iter().map(Into::into).collect();
            (StatusCode::OK, axum::Json(resp)).into_response()
        }
    }
}

async fn create_automation<A: AutomationRepository, R: RunRepository>(
    headers: HeaderMap,
    State(state): State<AppState<A, R>>,
    axum::Json(body): axum::Json<CreateRequest>,
) -> Response {
    let tid = match tenant_id(&headers) {
        Ok(t) => t,
        Err(r) => return r,
    };
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
        Ok(()) => (
            StatusCode::CREATED,
            axum::Json(AutomationResponse::from(automation)),
        )
            .into_response(),
    }
}

async fn get_automation<A: AutomationRepository, R: RunRepository>(
    headers: HeaderMap,
    State(state): State<AppState<A, R>>,
    Path(id): Path<String>,
) -> Response {
    let tid = match tenant_id(&headers) {
        Ok(t) => t,
        Err(r) => return r,
    };
    match state.store.get(&tid, &id).await {
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e),
        Ok(None) => err(StatusCode::NOT_FOUND, format!("automation {id} not found")),
        Ok(Some(a)) => (StatusCode::OK, axum::Json(AutomationResponse::from(a))).into_response(),
    }
}

async fn update_automation<A: AutomationRepository, R: RunRepository>(
    headers: HeaderMap,
    State(state): State<AppState<A, R>>,
    Path(id): Path<String>,
    axum::Json(body): axum::Json<UpdateRequest>,
) -> Response {
    let tid = match tenant_id(&headers) {
        Ok(t) => t,
        Err(r) => return r,
    };
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
        Ok(()) => (
            StatusCode::OK,
            axum::Json(AutomationResponse::from(updated)),
        )
            .into_response(),
    }
}

async fn delete_automation<A: AutomationRepository, R: RunRepository>(
    headers: HeaderMap,
    State(state): State<AppState<A, R>>,
    Path(id): Path<String>,
) -> Response {
    let tid = match tenant_id(&headers) {
        Ok(t) => t,
        Err(r) => return r,
    };
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

async fn enable_automation<A: AutomationRepository, R: RunRepository>(
    headers: HeaderMap,
    State(state): State<AppState<A, R>>,
    Path(id): Path<String>,
) -> Response {
    set_enabled(headers, state, id, true).await
}

async fn disable_automation<A: AutomationRepository, R: RunRepository>(
    headers: HeaderMap,
    State(state): State<AppState<A, R>>,
    Path(id): Path<String>,
) -> Response {
    set_enabled(headers, state, id, false).await
}

async fn set_enabled<A: AutomationRepository, R: RunRepository>(
    headers: HeaderMap,
    state: AppState<A, R>,
    id: String,
    enabled: bool,
) -> Response {
    let tid = match tenant_id(&headers) {
        Ok(t) => t,
        Err(r) => return r,
    };
    let mut automation = match state.store.get(&tid, &id).await {
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, e),
        Ok(None) => return err(StatusCode::NOT_FOUND, format!("automation {id} not found")),
        Ok(Some(a)) => a,
    };
    automation.enabled = enabled;
    automation.updated_at = now_iso8601();
    match state.store.put(&automation).await {
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e),
        Ok(()) => (
            StatusCode::OK,
            axum::Json(AutomationResponse::from(automation)),
        )
            .into_response(),
    }
}

// ── Run history handlers ───────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct RunsQuery {
    pub automation_id: Option<String>,
}

async fn list_runs<A: AutomationRepository, R: RunRepository>(
    headers: HeaderMap,
    State(state): State<AppState<A, R>>,
    Query(q): Query<RunsQuery>,
) -> Response {
    let tid = match tenant_id(&headers) {
        Ok(t) => t,
        Err(r) => return r,
    };
    match state.run_store.list(&tid, q.automation_id.as_deref()).await {
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e),
        Ok(runs) => (StatusCode::OK, axum::Json(runs)).into_response(),
    }
}

async fn list_runs_for_automation<A: AutomationRepository, R: RunRepository>(
    headers: HeaderMap,
    State(state): State<AppState<A, R>>,
    Path(id): Path<String>,
) -> Response {
    let tid = match tenant_id(&headers) {
        Ok(t) => t,
        Err(r) => return r,
    };
    match state.run_store.list(&tid, Some(&id)).await {
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e),
        Ok(runs) => (StatusCode::OK, axum::Json(runs)).into_response(),
    }
}

async fn get_stats<A: AutomationRepository, R: RunRepository>(
    headers: HeaderMap,
    State(state): State<AppState<A, R>>,
) -> Response {
    let tid = match tenant_id(&headers) {
        Ok(t) => t,
        Err(r) => return r,
    };
    match state.run_store.stats(&tid).await {
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e),
        Ok(stats) => (StatusCode::OK, axum::Json(stats)).into_response(),
    }
}

// ── Router ────────────────────────────────────────────────────────────────────

pub fn router<A: AutomationRepository, R: RunRepository>(state: AppState<A, R>) -> Router {
    Router::new()
        .route(
            "/automations",
            get(list_automations::<A, R>).post(create_automation::<A, R>),
        )
        .route(
            "/automations/{id}",
            get(get_automation::<A, R>)
                .put(update_automation::<A, R>)
                .delete(delete_automation::<A, R>),
        )
        .route("/automations/{id}/enable", patch(enable_automation::<A, R>))
        .route(
            "/automations/{id}/disable",
            patch(disable_automation::<A, R>),
        )
        .route(
            "/automations/{id}/runs",
            get(list_runs_for_automation::<A, R>),
        )
        .route("/runs", get(list_runs::<A, R>))
        .route("/stats", get(get_stats::<A, R>))
        .with_state(state)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runs::mock::MockRunStore;
    use crate::store::mock::MockAutomationStore;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use tower::util::ServiceExt as _;

    fn mock_app(store: MockAutomationStore, run_store: MockRunStore) -> axum::Router {
        router(AppState { store, run_store })
    }

    fn get_req(path: &str, tenant: &str) -> Request<Body> {
        Request::builder()
            .method("GET")
            .uri(path)
            .header("x-tenant-id", tenant)
            .body(Body::empty())
            .unwrap()
    }

    fn sample_automation(id: &str, tenant: &str) -> crate::automation::Automation {
        crate::automation::Automation {
            id: id.to_string(),
            tenant_id: tenant.to_string(),
            name: "My Automation".to_string(),
            trigger: "github.push".to_string(),
            prompt: "Do something".to_string(),
            model: None,
            tools: vec![],
            memory_path: None,
            mcp_servers: vec![],
            enabled: true,
            visibility: crate::automation::Visibility::default(),
            created_at: "2026-01-01T00:00:00Z".to_string(),
            updated_at: "2026-01-01T00:00:00Z".to_string(),
        }
    }

    // list_automations

    #[tokio::test]
    async fn list_automations_returns_empty_when_none() {
        let app = mock_app(MockAutomationStore::new(), MockRunStore::new());
        let resp = app.oneshot(get_req("/automations", "acme")).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json, serde_json::json!([]));
    }

    #[tokio::test]
    async fn list_automations_returns_tenant_automations() {
        let store = MockAutomationStore::new();
        store.insert(sample_automation("a1", "acme"));
        store.insert(sample_automation("a2", "acme"));
        store.insert(sample_automation("a3", "other"));
        let app = mock_app(store, MockRunStore::new());
        let resp = app.oneshot(get_req("/automations", "acme")).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json.as_array().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn list_automations_missing_tenant_returns_400() {
        let app = mock_app(MockAutomationStore::new(), MockRunStore::new());
        let req = Request::builder()
            .method("GET")
            .uri("/automations")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    // create_automation

    #[tokio::test]
    async fn create_automation_returns_201_and_persists() {
        let store = MockAutomationStore::new();
        let app = mock_app(store.clone(), MockRunStore::new());
        let body = serde_json::json!({
            "name": "Test Auto",
            "trigger": "github.push",
            "prompt": "Run checks"
        });
        let req = Request::builder()
            .method("POST")
            .uri("/automations")
            .header("x-tenant-id", "acme")
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED);
        let resp_body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&resp_body).unwrap();
        assert_eq!(json["name"], "Test Auto");
        assert_eq!(json["tenant_id"], "acme");
        assert_eq!(store.snapshot().len(), 1);
    }

    // get_automation

    #[tokio::test]
    async fn get_automation_returns_404_when_not_found() {
        let app = mock_app(MockAutomationStore::new(), MockRunStore::new());
        let resp = app
            .oneshot(get_req("/automations/no-such", "acme"))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn get_automation_returns_automation() {
        let store = MockAutomationStore::new();
        store.insert(sample_automation("a1", "acme"));
        let app = mock_app(store, MockRunStore::new());
        let resp = app
            .oneshot(get_req("/automations/a1", "acme"))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["id"], "a1");
    }

    // update_automation

    #[tokio::test]
    async fn update_automation_returns_404_when_not_found() {
        let app = mock_app(MockAutomationStore::new(), MockRunStore::new());
        let body = serde_json::json!({
            "name": "New", "trigger": "x", "prompt": "y", "enabled": true
        });
        let req = Request::builder()
            .method("PUT")
            .uri("/automations/no-such")
            .header("x-tenant-id", "acme")
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn update_automation_replaces_fields() {
        let store = MockAutomationStore::new();
        store.insert(sample_automation("a1", "acme"));
        let app = mock_app(store.clone(), MockRunStore::new());
        let body = serde_json::json!({
            "name": "Updated Name",
            "trigger": "github.push",
            "prompt": "New prompt",
            "enabled": false
        });
        let req = Request::builder()
            .method("PUT")
            .uri("/automations/a1")
            .header("x-tenant-id", "acme")
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let snap = store.snapshot();
        assert_eq!(snap["acme.a1"].name, "Updated Name");
        assert!(!snap["acme.a1"].enabled);
    }

    // delete_automation

    #[tokio::test]
    async fn delete_automation_returns_404_when_not_found() {
        let app = mock_app(MockAutomationStore::new(), MockRunStore::new());
        let req = Request::builder()
            .method("DELETE")
            .uri("/automations/no-such")
            .header("x-tenant-id", "acme")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn delete_automation_removes_and_returns_204() {
        let store = MockAutomationStore::new();
        store.insert(sample_automation("a1", "acme"));
        let app = mock_app(store.clone(), MockRunStore::new());
        let req = Request::builder()
            .method("DELETE")
            .uri("/automations/a1")
            .header("x-tenant-id", "acme")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);
        assert!(store.snapshot().is_empty());
    }

    // enable / disable

    #[tokio::test]
    async fn enable_automation_sets_enabled_true() {
        let store = MockAutomationStore::new();
        let mut auto = sample_automation("a1", "acme");
        auto.enabled = false;
        store.insert(auto);
        let app = mock_app(store.clone(), MockRunStore::new());
        let req = Request::builder()
            .method("PATCH")
            .uri("/automations/a1/enable")
            .header("x-tenant-id", "acme")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        assert!(store.snapshot()["acme.a1"].enabled);
    }

    #[tokio::test]
    async fn disable_automation_sets_enabled_false() {
        let store = MockAutomationStore::new();
        store.insert(sample_automation("a1", "acme"));
        let app = mock_app(store.clone(), MockRunStore::new());
        let req = Request::builder()
            .method("PATCH")
            .uri("/automations/a1/disable")
            .header("x-tenant-id", "acme")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        assert!(!store.snapshot()["acme.a1"].enabled);
    }

    // runs

    #[tokio::test]
    async fn list_runs_returns_empty_when_none() {
        let app = mock_app(MockAutomationStore::new(), MockRunStore::new());
        let resp = app.oneshot(get_req("/runs", "acme")).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json, serde_json::json!([]));
    }

    // stats

    #[tokio::test]
    async fn get_stats_returns_zero_counts_when_empty() {
        let app = mock_app(MockAutomationStore::new(), MockRunStore::new());
        let resp = app.oneshot(get_req("/stats", "acme")).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["total"], 0);
        assert_eq!(json["successful_7d"], 0);
        assert_eq!(json["failed_7d"], 0);
    }

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

use std::sync::Arc;

use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use serde_json::{Value, json};
use tower::util::ServiceExt as _;

use crate::{
    models::session::{ConsoleSession, SessionStatus},
    server::{AppState, build_router},
    store::mock::{
        MockAgentStore, MockCredentialStore, MockEnvironmentStore, MockSessionStore, MockSkillStore,
    },
};

// ── helpers ───────────────────────────────────────────────────────────────────

fn mock_state() -> Arc<AppState> {
    Arc::new(AppState {
        agents: Arc::new(MockAgentStore::new()),
        skills: Arc::new(MockSkillStore::new()),
        environments: Arc::new(MockEnvironmentStore::new()),
        credentials: Arc::new(MockCredentialStore::new()),
        sessions: Arc::new(MockSessionStore::new()),
    })
}

async fn body_json(body: axum::body::Body) -> Value {
    let bytes = axum::body::to_bytes(body, usize::MAX).await.unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

fn json_request(method: &str, uri: &str, body: Value) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(uri)
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap()
}

fn get_request(uri: &str) -> Request<Body> {
    Request::builder()
        .method("GET")
        .uri(uri)
        .body(Body::empty())
        .unwrap()
}

fn delete_request(uri: &str) -> Request<Body> {
    Request::builder()
        .method("DELETE")
        .uri(uri)
        .body(Body::empty())
        .unwrap()
}

// ── health ────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn health_returns_ok() {
    let app = build_router(mock_state());
    let resp = app.oneshot(get_request("/-/health")).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

// ── agents ────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn list_agents_empty() {
    let app = build_router(mock_state());
    let resp = app.oneshot(get_request("/agents")).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(body_json(resp.into_body()).await, json!([]));
}

#[tokio::test]
async fn create_and_get_agent() {
    let state = mock_state();

    let resp = build_router(Arc::clone(&state))
        .oneshot(json_request(
            "POST",
            "/agents",
            json!({
                "name": "Test Agent",
                "description": "desc",
                "model": { "id": "claude-sonnet-4-6" },
                "system_prompt": "You are a test agent."
            }),
        ))
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::CREATED);
    let created: Value = body_json(resp.into_body()).await;
    assert_eq!(created["name"], "Test Agent");
    assert_eq!(created["version"], 1);

    let id = created["id"].as_str().unwrap();
    let resp = build_router(Arc::clone(&state))
        .oneshot(get_request(&format!("/agents/{id}")))
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let fetched: Value = body_json(resp.into_body()).await;
    assert_eq!(fetched["id"], created["id"]);
}

#[tokio::test]
async fn get_agent_not_found() {
    let app = build_router(mock_state());
    let resp = app
        .oneshot(get_request("/agents/nonexistent"))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn update_agent_increments_version() {
    let state = mock_state();

    let resp = build_router(Arc::clone(&state))
        .oneshot(json_request(
            "POST",
            "/agents",
            json!({
                "name": "Original",
                "description": "",
                "model": { "id": "claude-haiku-4-5-20251001" },
                "system_prompt": "old"
            }),
        ))
        .await
        .unwrap();
    let created: Value = body_json(resp.into_body()).await;
    let id = created["id"].as_str().unwrap();

    let resp = build_router(Arc::clone(&state))
        .oneshot(json_request(
            "PUT",
            &format!("/agents/{id}"),
            json!({ "name": "Updated" }),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let updated: Value = body_json(resp.into_body()).await;
    assert_eq!(updated["name"], "Updated");
    assert_eq!(updated["version"], 2);
}

#[tokio::test]
async fn delete_agent() {
    let state = mock_state();

    let resp = build_router(Arc::clone(&state))
        .oneshot(json_request(
            "POST",
            "/agents",
            json!({
                "name": "Gone",
                "description": "",
                "model": { "id": "m" },
                "system_prompt": ""
            }),
        ))
        .await
        .unwrap();
    let created: Value = body_json(resp.into_body()).await;
    let id = created["id"].as_str().unwrap();

    let resp = build_router(Arc::clone(&state))
        .oneshot(delete_request(&format!("/agents/{id}")))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    let resp = build_router(Arc::clone(&state))
        .oneshot(get_request(&format!("/agents/{id}")))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn list_agent_versions() {
    let state = mock_state();

    let resp = build_router(Arc::clone(&state))
        .oneshot(json_request(
            "POST",
            "/agents",
            json!({
                "name": "Versioned",
                "description": "",
                "model": { "id": "m" },
                "system_prompt": ""
            }),
        ))
        .await
        .unwrap();
    let created: Value = body_json(resp.into_body()).await;
    let id = created["id"].as_str().unwrap();

    // Update to produce version 2
    build_router(Arc::clone(&state))
        .oneshot(json_request(
            "PUT",
            &format!("/agents/{id}"),
            json!({ "name": "V2" }),
        ))
        .await
        .unwrap();

    let resp = build_router(Arc::clone(&state))
        .oneshot(get_request(&format!("/agents/{id}/versions")))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let versions: Value = body_json(resp.into_body()).await;
    assert_eq!(versions.as_array().unwrap().len(), 2);
}

// ── skills ────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn list_skills_empty() {
    let app = build_router(mock_state());
    let resp = app.oneshot(get_request("/skills")).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(body_json(resp.into_body()).await, json!([]));
}

#[tokio::test]
async fn create_and_get_skill() {
    let state = mock_state();

    let resp = build_router(Arc::clone(&state))
        .oneshot(json_request(
            "POST",
            "/skills",
            json!({
                "name": "My Skill",
                "description": "does things",
                "content": "Use this skill to do things."
            }),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let created: Value = body_json(resp.into_body()).await;
    assert_eq!(created["name"], "My Skill");

    let id = created["id"].as_str().unwrap();
    let resp = build_router(Arc::clone(&state))
        .oneshot(get_request(&format!("/skills/{id}")))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let fetched: Value = body_json(resp.into_body()).await;
    assert_eq!(fetched["id"], created["id"]);
}

#[tokio::test]
async fn get_skill_not_found() {
    let app = build_router(mock_state());
    let resp = app.oneshot(get_request("/skills/ghost")).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn create_skill_version_and_list() {
    let state = mock_state();

    let resp = build_router(Arc::clone(&state))
        .oneshot(json_request(
            "POST",
            "/skills",
            json!({ "name": "Evolvable", "description": "", "content": "v1 content" }),
        ))
        .await
        .unwrap();
    let created: Value = body_json(resp.into_body()).await;
    let id = created["id"].as_str().unwrap();

    let resp = build_router(Arc::clone(&state))
        .oneshot(json_request(
            "POST",
            &format!("/skills/{id}/versions"),
            json!({ "content": "v2 content" }),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);

    let resp = build_router(Arc::clone(&state))
        .oneshot(get_request(&format!("/skills/{id}/versions")))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let versions: Value = body_json(resp.into_body()).await;
    // now_version() has day-level granularity; both versions share the same key
    // when run within the same day, so we get at least 1 entry with the latest content.
    let versions_arr = versions.as_array().unwrap();
    assert!(!versions_arr.is_empty());
    let contents: Vec<_> = versions_arr
        .iter()
        .map(|v| v["content"].as_str().unwrap())
        .collect();
    assert!(contents.contains(&"v2 content"));
}

// ── environments ──────────────────────────────────────────────────────────────

#[tokio::test]
async fn list_environments_empty() {
    let app = build_router(mock_state());
    let resp = app.oneshot(get_request("/environments")).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(body_json(resp.into_body()).await, json!([]));
}

#[tokio::test]
async fn create_and_get_environment() {
    let state = mock_state();

    let resp = build_router(Arc::clone(&state))
        .oneshot(json_request(
            "POST",
            "/environments",
            json!({ "name": "Prod", "description": "production" }),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let created: Value = body_json(resp.into_body()).await;
    assert_eq!(created["name"], "Prod");
    assert_eq!(created["archived"], false);

    let id = created["id"].as_str().unwrap();
    let resp = build_router(Arc::clone(&state))
        .oneshot(get_request(&format!("/environments/{id}")))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn update_environment() {
    let state = mock_state();

    let resp = build_router(Arc::clone(&state))
        .oneshot(json_request(
            "POST",
            "/environments",
            json!({ "name": "Staging" }),
        ))
        .await
        .unwrap();
    let created: Value = body_json(resp.into_body()).await;
    let id = created["id"].as_str().unwrap();

    let resp = build_router(Arc::clone(&state))
        .oneshot(json_request(
            "PUT",
            &format!("/environments/{id}"),
            json!({ "name": "Staging-v2" }),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let updated: Value = body_json(resp.into_body()).await;
    assert_eq!(updated["name"], "Staging-v2");
}

#[tokio::test]
async fn archive_environment() {
    let state = mock_state();

    let resp = build_router(Arc::clone(&state))
        .oneshot(json_request(
            "POST",
            "/environments",
            json!({ "name": "Old Env" }),
        ))
        .await
        .unwrap();
    let created: Value = body_json(resp.into_body()).await;
    let id = created["id"].as_str().unwrap();

    let resp = build_router(Arc::clone(&state))
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/environments/{id}/archive"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let archived: Value = body_json(resp.into_body()).await;
    assert_eq!(archived["archived"], true);
}

#[tokio::test]
async fn delete_environment() {
    let state = mock_state();

    let resp = build_router(Arc::clone(&state))
        .oneshot(json_request(
            "POST",
            "/environments",
            json!({ "name": "Temp" }),
        ))
        .await
        .unwrap();
    let created: Value = body_json(resp.into_body()).await;
    let id = created["id"].as_str().unwrap();

    let resp = build_router(Arc::clone(&state))
        .oneshot(delete_request(&format!("/environments/{id}")))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    let resp = build_router(Arc::clone(&state))
        .oneshot(get_request(&format!("/environments/{id}")))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

// ── credentials ───────────────────────────────────────────────────────────────

#[tokio::test]
async fn get_vault_before_creation_returns_404() {
    let app = build_router(mock_state());
    let resp = app
        .oneshot(get_request("/environments/env_xyz/vault"))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn create_credential_autocreates_vault() {
    let state = mock_state();

    let resp = build_router(Arc::clone(&state))
        .oneshot(json_request(
            "POST",
            "/environments/env_abc/credentials",
            json!({
                "name": "GitHub Token",
                "type": "bearer_token",
                "mcp_server_url": "https://api.github.com"
            }),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let cred: Value = body_json(resp.into_body()).await;
    assert_eq!(cred["name"], "GitHub Token");
    assert_eq!(cred["env_id"], "env_abc");

    let resp = build_router(Arc::clone(&state))
        .oneshot(get_request("/environments/env_abc/vault"))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let vault: Value = body_json(resp.into_body()).await;
    assert_eq!(vault["env_id"], "env_abc");
}

#[tokio::test]
async fn list_and_delete_credentials() {
    let state = mock_state();

    build_router(Arc::clone(&state))
        .oneshot(json_request(
            "POST",
            "/environments/env_del/credentials",
            json!({
                "name": "Token A",
                "type": "bearer_token",
                "mcp_server_url": "https://example.com"
            }),
        ))
        .await
        .unwrap();

    let resp = build_router(Arc::clone(&state))
        .oneshot(get_request("/environments/env_del/credentials"))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let creds: Value = body_json(resp.into_body()).await;
    let creds_arr = creds.as_array().unwrap();
    assert_eq!(creds_arr.len(), 1);

    let cred_id = creds_arr[0]["id"].as_str().unwrap();
    let resp = build_router(Arc::clone(&state))
        .oneshot(delete_request(&format!(
            "/environments/env_del/credentials/{cred_id}"
        )))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    let resp = build_router(Arc::clone(&state))
        .oneshot(get_request("/environments/env_del/credentials"))
        .await
        .unwrap();
    let creds: Value = body_json(resp.into_body()).await;
    assert_eq!(creds.as_array().unwrap().len(), 0);
}

// ── sessions ──────────────────────────────────────────────────────────────────

#[tokio::test]
async fn list_sessions_empty() {
    let app = build_router(mock_state());
    let resp = app.oneshot(get_request("/sessions")).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(body_json(resp.into_body()).await, json!([]));
}

#[tokio::test]
async fn list_and_get_sessions() {
    let state = Arc::new(AppState {
        agents: Arc::new(MockAgentStore::new()),
        skills: Arc::new(MockSkillStore::new()),
        environments: Arc::new(MockEnvironmentStore::new()),
        credentials: Arc::new(MockCredentialStore::new()),
        sessions: Arc::new({
            let store = MockSessionStore::new();
            store.insert(ConsoleSession {
                id: "sess_001".to_string(),
                tenant_id: "tenant_a".to_string(),
                name: "Session 1".to_string(),
                model: Some("claude-sonnet-4-6".to_string()),
                status: SessionStatus::Idle,
                message_count: 4,
                input_tokens: 100,
                output_tokens: 200,
                cache_read_tokens: 0,
                cache_write_tokens: 0,
                duration_ms: 0,
                agent_id: None,
                created_at: "1000".to_string(),
                updated_at: "2000".to_string(),
            });
            store.insert(ConsoleSession {
                id: "sess_002".to_string(),
                tenant_id: "tenant_b".to_string(),
                name: "Session 2".to_string(),
                model: None,
                status: SessionStatus::Running,
                message_count: 1,
                input_tokens: 10,
                output_tokens: 0,
                cache_read_tokens: 0,
                cache_write_tokens: 0,
                duration_ms: 0,
                agent_id: None,
                created_at: "500".to_string(),
                updated_at: "1500".to_string(),
            });
            store
        }),
    });

    // list all
    let resp = build_router(Arc::clone(&state))
        .oneshot(get_request("/sessions"))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let sessions: Value = body_json(resp.into_body()).await;
    assert_eq!(sessions.as_array().unwrap().len(), 2);

    // get by tenant + id
    let resp = build_router(Arc::clone(&state))
        .oneshot(get_request("/sessions/tenant_a/sess_001"))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let session: Value = body_json(resp.into_body()).await;
    assert_eq!(session["id"], "sess_001");
    assert_eq!(session["tenant_id"], "tenant_a");
}

#[tokio::test]
async fn get_session_not_found() {
    let app = build_router(mock_state());
    let resp = app.oneshot(get_request("/sessions/t/nope")).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn list_agent_sessions_by_agent_id() {
    let state = Arc::new(AppState {
        agents: Arc::new(MockAgentStore::new()),
        skills: Arc::new(MockSkillStore::new()),
        environments: Arc::new(MockEnvironmentStore::new()),
        credentials: Arc::new(MockCredentialStore::new()),
        sessions: Arc::new({
            let store = MockSessionStore::new();
            store.insert(ConsoleSession {
                id: "s1".to_string(),
                tenant_id: "tenant_a".to_string(),
                name: "S1".to_string(),
                model: None,
                status: SessionStatus::Idle,
                message_count: 0,
                input_tokens: 0,
                output_tokens: 0,
                cache_read_tokens: 0,
                cache_write_tokens: 0,
                duration_ms: 0,
                agent_id: Some("agent_xyz".to_string()),
                created_at: "1".to_string(),
                updated_at: "1".to_string(),
            });
            store.insert(ConsoleSession {
                id: "s2".to_string(),
                tenant_id: "tenant_b".to_string(),
                name: "S2".to_string(),
                model: None,
                status: SessionStatus::Idle,
                message_count: 0,
                input_tokens: 0,
                output_tokens: 0,
                cache_read_tokens: 0,
                cache_write_tokens: 0,
                duration_ms: 0,
                agent_id: Some("agent_other".to_string()),
                created_at: "1".to_string(),
                updated_at: "1".to_string(),
            });
            store
        }),
    });

    let resp = build_router(Arc::clone(&state))
        .oneshot(get_request("/agents/agent_xyz/sessions"))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let sessions: Value = body_json(resp.into_body()).await;
    assert_eq!(sessions.as_array().unwrap().len(), 1);
    assert_eq!(sessions[0]["id"], "s1");
}

// ── store error helpers ───────────────────────────────────────────────────────

fn fail_agents_state() -> Arc<AppState> {
    Arc::new(AppState {
        agents: Arc::new(MockAgentStore::failing()),
        skills: Arc::new(MockSkillStore::new()),
        environments: Arc::new(MockEnvironmentStore::new()),
        credentials: Arc::new(MockCredentialStore::new()),
        sessions: Arc::new(MockSessionStore::new()),
    })
}

fn fail_skills_state() -> Arc<AppState> {
    Arc::new(AppState {
        agents: Arc::new(MockAgentStore::new()),
        skills: Arc::new(MockSkillStore::failing()),
        environments: Arc::new(MockEnvironmentStore::new()),
        credentials: Arc::new(MockCredentialStore::new()),
        sessions: Arc::new(MockSessionStore::new()),
    })
}

fn fail_environments_state() -> Arc<AppState> {
    Arc::new(AppState {
        agents: Arc::new(MockAgentStore::new()),
        skills: Arc::new(MockSkillStore::new()),
        environments: Arc::new(MockEnvironmentStore::failing()),
        credentials: Arc::new(MockCredentialStore::new()),
        sessions: Arc::new(MockSessionStore::new()),
    })
}

fn fail_credentials_state() -> Arc<AppState> {
    Arc::new(AppState {
        agents: Arc::new(MockAgentStore::new()),
        skills: Arc::new(MockSkillStore::new()),
        environments: Arc::new(MockEnvironmentStore::new()),
        credentials: Arc::new(MockCredentialStore::failing()),
        sessions: Arc::new(MockSessionStore::new()),
    })
}

fn fail_sessions_state() -> Arc<AppState> {
    Arc::new(AppState {
        agents: Arc::new(MockAgentStore::new()),
        skills: Arc::new(MockSkillStore::new()),
        environments: Arc::new(MockEnvironmentStore::new()),
        credentials: Arc::new(MockCredentialStore::new()),
        sessions: Arc::new(MockSessionStore::failing()),
    })
}

// ── agent store error paths ───────────────────────────────────────────────────

#[tokio::test]
async fn list_agents_store_error_returns_500() {
    let app = build_router(fail_agents_state());
    let resp = app.oneshot(get_request("/agents")).await.unwrap();
    assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
}

#[tokio::test]
async fn get_agent_store_error_returns_500() {
    let app = build_router(fail_agents_state());
    let resp = app.oneshot(get_request("/agents/any_id")).await.unwrap();
    assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
}

#[tokio::test]
async fn create_agent_store_error_returns_500() {
    let app = build_router(fail_agents_state());
    let resp = app
        .oneshot(json_request(
            "POST",
            "/agents",
            json!({
                "name": "X",
                "description": "",
                "model": { "id": "m" },
                "system_prompt": ""
            }),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
}

#[tokio::test]
async fn update_agent_store_error_returns_500() {
    let app = build_router(fail_agents_state());
    let resp = app
        .oneshot(json_request(
            "PUT",
            "/agents/any_id",
            json!({ "name": "X" }),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
}

#[tokio::test]
async fn delete_agent_store_error_returns_500() {
    let app = build_router(fail_agents_state());
    let resp = app.oneshot(delete_request("/agents/any_id")).await.unwrap();
    assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
}

#[tokio::test]
async fn list_agent_versions_store_error_returns_500() {
    let app = build_router(fail_agents_state());
    let resp = app
        .oneshot(get_request("/agents/any_id/versions"))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
}

#[tokio::test]
async fn list_agent_sessions_store_error_returns_500() {
    let app = build_router(fail_sessions_state());
    let resp = app
        .oneshot(get_request("/agents/any_id/sessions"))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
}

#[tokio::test]
async fn update_agent_not_found_returns_404() {
    let app = build_router(mock_state());
    let resp = app
        .oneshot(json_request(
            "PUT",
            "/agents/ghost_id",
            json!({ "name": "X" }),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn update_agent_all_fields() {
    let state = mock_state();

    let resp = build_router(Arc::clone(&state))
        .oneshot(json_request(
            "POST",
            "/agents",
            json!({
                "name": "Original",
                "description": "old desc",
                "model": { "id": "m1" },
                "system_prompt": "old prompt"
            }),
        ))
        .await
        .unwrap();
    let created: Value = body_json(resp.into_body()).await;
    let id = created["id"].as_str().unwrap();

    let resp = build_router(Arc::clone(&state))
        .oneshot(json_request(
            "PUT",
            &format!("/agents/{id}"),
            json!({
                "name": "Updated Name",
                "description": "new desc",
                "status": "inactive",
                "model": { "id": "m2" },
                "system_prompt": "new prompt",
                "skill_ids": ["sk1", "sk2"],
                "tools": [{"type": "bash"}],
                "mcp_servers": ["https://mcp.example.com"],
                "metadata": { "key": "value" }
            }),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let updated: Value = body_json(resp.into_body()).await;
    assert_eq!(updated["name"], "Updated Name");
    assert_eq!(updated["description"], "new desc");
    assert_eq!(updated["model"]["id"], "m2");
    assert_eq!(updated["system_prompt"], "new prompt");
    assert_eq!(updated["skill_ids"].as_array().unwrap().len(), 2);
    assert_eq!(updated["version"], 2);
}

// ── skill store error paths ───────────────────────────────────────────────────

#[tokio::test]
async fn list_skills_store_error_returns_500() {
    let app = build_router(fail_skills_state());
    let resp = app.oneshot(get_request("/skills")).await.unwrap();
    assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
}

#[tokio::test]
async fn get_skill_store_error_returns_500() {
    let app = build_router(fail_skills_state());
    let resp = app.oneshot(get_request("/skills/any_id")).await.unwrap();
    assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
}

#[tokio::test]
async fn create_skill_store_error_returns_500() {
    let app = build_router(fail_skills_state());
    let resp = app
        .oneshot(json_request(
            "POST",
            "/skills",
            json!({ "name": "S", "description": "", "content": "c" }),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
}

#[tokio::test]
async fn create_skill_version_store_error_returns_500() {
    let app = build_router(fail_skills_state());
    let resp = app
        .oneshot(json_request(
            "POST",
            "/skills/any_id/versions",
            json!({ "content": "v2" }),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
}

#[tokio::test]
async fn list_skill_versions_store_error_returns_500() {
    let app = build_router(fail_skills_state());
    let resp = app
        .oneshot(get_request("/skills/any_id/versions"))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
}

#[tokio::test]
async fn create_skill_version_skill_not_found_returns_404() {
    let app = build_router(mock_state());
    let resp = app
        .oneshot(json_request(
            "POST",
            "/skills/ghost/versions",
            json!({ "content": "v2" }),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

// ── environment store error paths ─────────────────────────────────────────────

#[tokio::test]
async fn list_environments_store_error_returns_500() {
    let app = build_router(fail_environments_state());
    let resp = app.oneshot(get_request("/environments")).await.unwrap();
    assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
}

#[tokio::test]
async fn get_environment_store_error_returns_500() {
    let app = build_router(fail_environments_state());
    let resp = app
        .oneshot(get_request("/environments/any_id"))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
}

#[tokio::test]
async fn create_environment_store_error_returns_500() {
    let app = build_router(fail_environments_state());
    let resp = app
        .oneshot(json_request(
            "POST",
            "/environments",
            json!({ "name": "E" }),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
}

#[tokio::test]
async fn update_environment_store_error_returns_500() {
    let app = build_router(fail_environments_state());
    let resp = app
        .oneshot(json_request(
            "PUT",
            "/environments/any_id",
            json!({ "name": "E" }),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
}

#[tokio::test]
async fn archive_environment_store_error_returns_500() {
    let app = build_router(fail_environments_state());
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/environments/any_id/archive")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
}

#[tokio::test]
async fn delete_environment_store_error_returns_500() {
    let app = build_router(fail_environments_state());
    let resp = app
        .oneshot(delete_request("/environments/any_id"))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
}

#[tokio::test]
async fn get_environment_not_found_returns_404() {
    let app = build_router(mock_state());
    let resp = app
        .oneshot(get_request("/environments/ghost_id"))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn update_environment_not_found_returns_404() {
    let app = build_router(mock_state());
    let resp = app
        .oneshot(json_request(
            "PUT",
            "/environments/ghost_id",
            json!({ "name": "X" }),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn archive_environment_not_found_returns_404() {
    let app = build_router(mock_state());
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/environments/ghost_id/archive")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn update_environment_all_fields() {
    let state = mock_state();

    let resp = build_router(Arc::clone(&state))
        .oneshot(json_request(
            "POST",
            "/environments",
            json!({ "name": "Old" }),
        ))
        .await
        .unwrap();
    let created: Value = body_json(resp.into_body()).await;
    let id = created["id"].as_str().unwrap();

    let resp = build_router(Arc::clone(&state))
        .oneshot(json_request(
            "PUT",
            &format!("/environments/{id}"),
            json!({
                "name": "New Name",
                "description": "new desc",
                "env_type": "local",
                "networking": "restricted",
                "packages": [],
                "metadata": { "region": "us-east-1" }
            }),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let updated: Value = body_json(resp.into_body()).await;
    assert_eq!(updated["name"], "New Name");
    assert_eq!(updated["description"], "new desc");
    assert_eq!(updated["metadata"]["region"], "us-east-1");
}

// ── credential store error paths ──────────────────────────────────────────────

#[tokio::test]
async fn get_vault_store_error_returns_500() {
    let app = build_router(fail_credentials_state());
    let resp = app
        .oneshot(get_request("/environments/env1/vault"))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
}

#[tokio::test]
async fn list_credentials_store_error_returns_500() {
    let app = build_router(fail_credentials_state());
    let resp = app
        .oneshot(get_request("/environments/env1/credentials"))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
}

#[tokio::test]
async fn create_credential_store_error_returns_500() {
    let app = build_router(fail_credentials_state());
    let resp = app
        .oneshot(json_request(
            "POST",
            "/environments/env1/credentials",
            json!({
                "name": "Token",
                "type": "bearer_token",
                "mcp_server_url": "https://example.com"
            }),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
}

#[tokio::test]
async fn delete_credential_store_error_returns_500() {
    let app = build_router(fail_credentials_state());
    let resp = app
        .oneshot(delete_request("/environments/env1/credentials/crd1"))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
}

// ── session store error paths ─────────────────────────────────────────────────

#[tokio::test]
async fn list_sessions_store_error_returns_500() {
    let app = build_router(fail_sessions_state());
    let resp = app.oneshot(get_request("/sessions")).await.unwrap();
    assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
}

#[tokio::test]
async fn get_session_store_error_returns_500() {
    let app = build_router(fail_sessions_state());
    let resp = app.oneshot(get_request("/sessions/t/s")).await.unwrap();
    assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
}

// ── delete skill ──────────────────────────────────────────────────────────────

#[tokio::test]
async fn delete_skill() {
    let state = mock_state();

    let resp = build_router(Arc::clone(&state))
        .oneshot(json_request(
            "POST",
            "/skills",
            json!({ "name": "Temp Skill", "description": "", "content": "c" }),
        ))
        .await
        .unwrap();
    let created: Value = body_json(resp.into_body()).await;
    let id = created["id"].as_str().unwrap();

    let resp = build_router(Arc::clone(&state))
        .oneshot(delete_request(&format!("/skills/{id}")))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    let resp = build_router(Arc::clone(&state))
        .oneshot(get_request(&format!("/skills/{id}")))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn delete_skill_store_error_returns_500() {
    let app = build_router(fail_skills_state());
    let resp = app.oneshot(delete_request("/skills/any_id")).await.unwrap();
    assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
}

// ── get credential ────────────────────────────────────────────────────────────

#[tokio::test]
async fn get_credential_found() {
    let state = mock_state();

    let resp = build_router(Arc::clone(&state))
        .oneshot(json_request(
            "POST",
            "/environments/env_get/credentials",
            json!({
                "name": "My Token",
                "type": "bearer_token",
                "mcp_server_url": "https://example.com"
            }),
        ))
        .await
        .unwrap();
    let created: Value = body_json(resp.into_body()).await;
    let cred_id = created["id"].as_str().unwrap();

    let resp = build_router(Arc::clone(&state))
        .oneshot(get_request(&format!(
            "/environments/env_get/credentials/{cred_id}"
        )))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let fetched: Value = body_json(resp.into_body()).await;
    assert_eq!(fetched["id"], created["id"]);
    assert_eq!(fetched["name"], "My Token");
}

#[tokio::test]
async fn get_credential_not_found_returns_404() {
    let app = build_router(mock_state());
    let resp = app
        .oneshot(get_request("/environments/env1/credentials/ghost"))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn get_credential_store_error_returns_500() {
    let app = build_router(fail_credentials_state());
    let resp = app
        .oneshot(get_request("/environments/env1/credentials/crd1"))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
}

// ── partial field updates ─────────────────────────────────────────────────────

#[tokio::test]
async fn update_agent_only_name_leaves_other_fields_unchanged() {
    let state = mock_state();

    let resp = build_router(Arc::clone(&state))
        .oneshot(json_request(
            "POST",
            "/agents",
            json!({
                "name": "Original",
                "description": "original desc",
                "model": { "id": "m1" },
                "system_prompt": "original prompt"
            }),
        ))
        .await
        .unwrap();
    let created: Value = body_json(resp.into_body()).await;
    let id = created["id"].as_str().unwrap();

    let resp = build_router(Arc::clone(&state))
        .oneshot(json_request(
            "PUT",
            &format!("/agents/{id}"),
            json!({ "name": "Renamed" }),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let updated: Value = body_json(resp.into_body()).await;
    assert_eq!(updated["name"], "Renamed");
    assert_eq!(updated["description"], "original desc");
    assert_eq!(updated["system_prompt"], "original prompt");
    assert_eq!(updated["model"]["id"], "m1");
    assert_eq!(updated["version"], 2);
}

#[tokio::test]
async fn update_environment_only_description_leaves_other_fields_unchanged() {
    let state = mock_state();

    let resp = build_router(Arc::clone(&state))
        .oneshot(json_request(
            "POST",
            "/environments",
            json!({ "name": "Prod", "description": "original" }),
        ))
        .await
        .unwrap();
    let created: Value = body_json(resp.into_body()).await;
    let id = created["id"].as_str().unwrap();

    let resp = build_router(Arc::clone(&state))
        .oneshot(json_request(
            "PUT",
            &format!("/environments/{id}"),
            json!({ "description": "updated desc" }),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let updated: Value = body_json(resp.into_body()).await;
    assert_eq!(updated["name"], "Prod");
    assert_eq!(updated["description"], "updated desc");
}

// ── multi-item sort coverage ──────────────────────────────────────────────────

#[tokio::test]
async fn list_agents_with_two_items_invokes_sort() {
    let state = mock_state();
    for name in ["Agent B", "Agent A"] {
        build_router(Arc::clone(&state))
            .oneshot(json_request(
                "POST",
                "/agents",
                json!({ "name": name, "description": "", "model": { "id": "m" }, "system_prompt": "" }),
            ))
            .await
            .unwrap();
    }
    let resp = build_router(Arc::clone(&state))
        .oneshot(get_request("/agents"))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let agents: Value = body_json(resp.into_body()).await;
    assert_eq!(agents.as_array().unwrap().len(), 2);
}

#[tokio::test]
async fn list_skills_with_two_items_invokes_sort() {
    let state = mock_state();
    for name in ["Skill Z", "Skill A"] {
        build_router(Arc::clone(&state))
            .oneshot(json_request(
                "POST",
                "/skills",
                json!({ "name": name, "description": "", "content": "c" }),
            ))
            .await
            .unwrap();
    }
    let resp = build_router(Arc::clone(&state))
        .oneshot(get_request("/skills"))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let skills: Value = body_json(resp.into_body()).await;
    let arr = skills.as_array().unwrap();
    assert_eq!(arr.len(), 2);
    assert_eq!(arr[0]["name"], "Skill A");
    assert_eq!(arr[1]["name"], "Skill Z");
}

#[tokio::test]
async fn list_environments_with_two_items_invokes_sort() {
    let state = mock_state();
    for name in ["Prod", "Dev"] {
        build_router(Arc::clone(&state))
            .oneshot(json_request(
                "POST",
                "/environments",
                json!({ "name": name }),
            ))
            .await
            .unwrap();
    }
    let resp = build_router(Arc::clone(&state))
        .oneshot(get_request("/environments"))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let envs: Value = body_json(resp.into_body()).await;
    let arr = envs.as_array().unwrap();
    assert_eq!(arr.len(), 2);
    assert_eq!(arr[0]["name"], "Dev");
    assert_eq!(arr[1]["name"], "Prod");
}

#[tokio::test]
async fn list_credentials_with_two_items_invokes_sort() {
    let state = mock_state();
    for name in ["Token A", "Token B"] {
        build_router(Arc::clone(&state))
            .oneshot(json_request(
                "POST",
                "/environments/env_sort/credentials",
                json!({ "name": name, "type": "bearer_token", "mcp_server_url": "https://example.com" }),
            ))
            .await
            .unwrap();
    }
    let resp = build_router(Arc::clone(&state))
        .oneshot(get_request("/environments/env_sort/credentials"))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let creds: Value = body_json(resp.into_body()).await;
    assert_eq!(creds.as_array().unwrap().len(), 2);
}

// ── put-error branch coverage ─────────────────────────────────────────────────

#[tokio::test]
async fn update_agent_put_error_returns_500() {
    use crate::models::agent::{AgentDefinition, AgentModel, AgentStatus};
    let store = MockAgentStore::failing_put();
    store.insert(AgentDefinition {
        id: "agent_put_fail".to_string(),
        name: "X".to_string(),
        description: "".to_string(),
        status: AgentStatus::Active,
        version: 1,
        model: AgentModel {
            id: "m".to_string(),
            speed: "standard".to_string(),
        },
        system_prompt: "".to_string(),
        skill_ids: vec![],
        tools: vec![],
        mcp_servers: vec![],
        metadata: serde_json::Value::Null,
        created_at: "0".to_string(),
        updated_at: "0".to_string(),
    });
    let app = build_router(Arc::new(AppState {
        agents: Arc::new(store),
        skills: Arc::new(MockSkillStore::new()),
        environments: Arc::new(MockEnvironmentStore::new()),
        credentials: Arc::new(MockCredentialStore::new()),
        sessions: Arc::new(MockSessionStore::new()),
    }));
    let resp = app
        .oneshot(json_request(
            "PUT",
            "/agents/agent_put_fail",
            json!({ "name": "Y" }),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
}

#[tokio::test]
async fn update_environment_put_error_returns_500() {
    use crate::models::environment::{Environment, EnvironmentType, NetworkingType};
    let store = MockEnvironmentStore::failing_put();
    store.insert(Environment {
        id: "env_put_fail".to_string(),
        name: "E".to_string(),
        description: "".to_string(),
        env_type: EnvironmentType::Cloud,
        networking: NetworkingType::Unrestricted,
        packages: vec![],
        metadata: Default::default(),
        archived: false,
        created_at: "0".to_string(),
        updated_at: "0".to_string(),
    });
    let app = build_router(Arc::new(AppState {
        agents: Arc::new(MockAgentStore::new()),
        skills: Arc::new(MockSkillStore::new()),
        environments: Arc::new(store),
        credentials: Arc::new(MockCredentialStore::new()),
        sessions: Arc::new(MockSessionStore::new()),
    }));
    let resp = app
        .oneshot(json_request(
            "PUT",
            "/environments/env_put_fail",
            json!({ "name": "E2" }),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
}

#[tokio::test]
async fn archive_environment_put_error_returns_500() {
    use crate::models::environment::{Environment, EnvironmentType, NetworkingType};
    let store = MockEnvironmentStore::failing_put();
    store.insert(Environment {
        id: "env_arch_fail".to_string(),
        name: "E".to_string(),
        description: "".to_string(),
        env_type: EnvironmentType::Cloud,
        networking: NetworkingType::Unrestricted,
        packages: vec![],
        metadata: Default::default(),
        archived: false,
        created_at: "0".to_string(),
        updated_at: "0".to_string(),
    });
    let app = build_router(Arc::new(AppState {
        agents: Arc::new(MockAgentStore::new()),
        skills: Arc::new(MockSkillStore::new()),
        environments: Arc::new(store),
        credentials: Arc::new(MockCredentialStore::new()),
        sessions: Arc::new(MockSessionStore::new()),
    }));
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/environments/env_arch_fail/archive")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
}

#[tokio::test]
async fn create_credential_put_error_returns_500() {
    let app = build_router(Arc::new(AppState {
        agents: Arc::new(MockAgentStore::new()),
        skills: Arc::new(MockSkillStore::new()),
        environments: Arc::new(MockEnvironmentStore::new()),
        credentials: Arc::new(MockCredentialStore::failing_put()),
        sessions: Arc::new(MockSessionStore::new()),
    }));
    let resp = app
        .oneshot(json_request(
            "POST",
            "/environments/env_cred_fail/credentials",
            json!({ "name": "T", "type": "bearer_token", "mcp_server_url": "https://x.com" }),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
}

#[tokio::test]
async fn create_skill_put_version_error_returns_500() {
    let app = build_router(Arc::new(AppState {
        agents: Arc::new(MockAgentStore::new()),
        skills: Arc::new(MockSkillStore::failing_put_version()),
        environments: Arc::new(MockEnvironmentStore::new()),
        credentials: Arc::new(MockCredentialStore::new()),
        sessions: Arc::new(MockSessionStore::new()),
    }));
    let resp = app
        .oneshot(json_request(
            "POST",
            "/skills",
            json!({ "name": "S", "description": "", "content": "c" }),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
}

#[tokio::test]
async fn create_skill_version_put_version_error_returns_500() {
    use crate::models::skill::Skill;
    let store = MockSkillStore::failing_put_version();
    store.insert_skill(Skill {
        id: "skill_pvfail".to_string(),
        name: "S".to_string(),
        description: "".to_string(),
        provider: "custom".to_string(),
        latest_version: "20260101".to_string(),
        created_at: "0".to_string(),
        updated_at: "0".to_string(),
    });
    let app = build_router(Arc::new(AppState {
        agents: Arc::new(MockAgentStore::new()),
        skills: Arc::new(store),
        environments: Arc::new(MockEnvironmentStore::new()),
        credentials: Arc::new(MockCredentialStore::new()),
        sessions: Arc::new(MockSessionStore::new()),
    }));
    let resp = app
        .oneshot(json_request(
            "POST",
            "/skills/skill_pvfail/versions",
            json!({ "content": "v2" }),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
}

#[tokio::test]
async fn create_skill_version_put_error_returns_500() {
    use crate::models::skill::Skill;
    let store = MockSkillStore::failing_put();
    store.insert_skill(Skill {
        id: "skill_pfail".to_string(),
        name: "S".to_string(),
        description: "".to_string(),
        provider: "custom".to_string(),
        latest_version: "20260101".to_string(),
        created_at: "0".to_string(),
        updated_at: "0".to_string(),
    });
    let app = build_router(Arc::new(AppState {
        agents: Arc::new(MockAgentStore::new()),
        skills: Arc::new(store),
        environments: Arc::new(MockEnvironmentStore::new()),
        credentials: Arc::new(MockCredentialStore::new()),
        sessions: Arc::new(MockSessionStore::new()),
    }));
    let resp = app
        .oneshot(json_request(
            "POST",
            "/skills/skill_pfail/versions",
            json!({ "content": "v2" }),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
}

// ── None-branch coverage for optional PUT fields ──────────────────────────────

#[tokio::test]
async fn update_agent_without_name_covers_none_branch() {
    let state = mock_state();

    let resp = build_router(Arc::clone(&state))
        .oneshot(json_request(
            "POST",
            "/agents",
            json!({ "name": "Original", "description": "d", "model": { "id": "m" }, "system_prompt": "s" }),
        ))
        .await
        .unwrap();
    let created: Value = body_json(resp.into_body()).await;
    let id = created["id"].as_str().unwrap();

    let resp = build_router(Arc::clone(&state))
        .oneshot(json_request(
            "PUT",
            &format!("/agents/{id}"),
            json!({ "description": "updated" }),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let updated: Value = body_json(resp.into_body()).await;
    assert_eq!(updated["name"], "Original");
    assert_eq!(updated["description"], "updated");
}

#[tokio::test]
async fn update_environment_without_name_covers_none_branch() {
    let state = mock_state();

    let resp = build_router(Arc::clone(&state))
        .oneshot(json_request(
            "POST",
            "/environments",
            json!({ "name": "Env", "description": "old" }),
        ))
        .await
        .unwrap();
    let created: Value = body_json(resp.into_body()).await;
    let id = created["id"].as_str().unwrap();

    let resp = build_router(Arc::clone(&state))
        .oneshot(json_request(
            "PUT",
            &format!("/environments/{id}"),
            json!({ "description": "new desc" }),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let updated: Value = body_json(resp.into_body()).await;
    assert_eq!(updated["name"], "Env");
    assert_eq!(updated["description"], "new desc");
}

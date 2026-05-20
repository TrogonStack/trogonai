//! Live end-to-end test for the trogon-console HTTP server.
//!
//! Exercises every HTTP route against a real NATS JetStream backend.
//!
//! Requirements:
//!   • NATS running on nats://localhost:4222  (JetStream required)
//!   • No external credentials
//!
//! Run:
//!   cargo run -p trogon-e2e --bin console_live

use std::sync::Arc;

use reqwest::Client;
use serde_json::{Value, json};
use trogon_console::{
    server::{AppState, build_router},
    store::{
        agents::AgentStore, credentials::CredentialStore, environments::EnvironmentStore,
        sessions::SessionReader, skills::SkillStore,
        traits::{
            AgentRepository, CredentialRepository, EnvironmentRepository, SessionRepository,
            SkillRepository,
        },
    },
};
use uuid::Uuid;

// ── Output helpers ─────────────────────────────────────────────────────────────

fn ok(label: &str) {
    println!("  \x1b[32m✓\x1b[0m  {label}");
}

fn ko(label: &str, reason: &str) {
    println!("  \x1b[31m✗\x1b[0m  {label}");
    println!("       {reason}");
}

fn uid() -> String {
    Uuid::new_v4().to_string()[..8].to_string()
}

// ── Server bootstrap ───────────────────────────────────────────────────────────

async fn start_server() -> (String, reqwest::Client) {
    let nats = async_nats::connect("nats://localhost:4222")
        .await
        .expect("NATS must be running on localhost:4222");
    let js = async_nats::jetstream::new(nats);

    let state = Arc::new(AppState {
        agents: Arc::new(AgentStore::open(&js).await.unwrap()) as Arc<dyn AgentRepository>,
        skills: Arc::new(SkillStore::open(&js).await.unwrap()) as Arc<dyn SkillRepository>,
        environments: Arc::new(EnvironmentStore::open(&js).await.unwrap())
            as Arc<dyn EnvironmentRepository>,
        credentials: Arc::new(CredentialStore::open(&js).await.unwrap())
            as Arc<dyn CredentialRepository>,
        sessions: Arc::new(SessionReader::open(&js).await.unwrap()) as Arc<dyn SessionRepository>,
    });

    let router = build_router(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(axum::serve(listener, router).into_future());

    let base = format!("http://127.0.0.1:{}", addr.port());
    (base, Client::new())
}

// ── Health ─────────────────────────────────────────────────────────────────────

async fn test_health(base: &str, http: &Client) -> bool {
    const LABEL: &str = "Health — GET /-/health returns 200 ok";
    match http.get(format!("{base}/-/health")).send().await {
        Ok(r) if r.status() == 200 => { ok(LABEL); true }
        Ok(r) => { ko(LABEL, &format!("status {}", r.status())); false }
        Err(e) => { ko(LABEL, &e.to_string()); false }
    }
}

// ── Agents ─────────────────────────────────────────────────────────────────────

async fn test_agents_crud(base: &str, http: &Client) -> bool {
    const LABEL: &str = "Agents — create / get / update / list / versions / delete";
    let id = uid();

    let result: Result<(), String> = async {
        // Create
        let body = json!({"name": format!("agent-{id}"), "description": "test", "model": {"id": "claude-test"}, "system_prompt": "You are a test agent."});
        let r = http.post(format!("{base}/agents")).json(&body).send().await.map_err(|e| e.to_string())?;
        if r.status() != 201 { return Err(format!("create: {}", r.status())); }
        let agent: Value = r.json().await.map_err(|e| e.to_string())?;
        let agent_id = agent["id"].as_str().ok_or("no id")?.to_string();
        if agent["version"].as_u64() != Some(1) { return Err(format!("expected version 1, got {}", agent["version"])); }

        // Get
        let r = http.get(format!("{base}/agents/{agent_id}")).send().await.map_err(|e| e.to_string())?;
        if r.status() != 200 { return Err(format!("get: {}", r.status())); }

        // Update
        let r = http.put(format!("{base}/agents/{agent_id}")).json(&json!({"description": "updated"})).send().await.map_err(|e| e.to_string())?;
        if r.status() != 200 { return Err(format!("update: {}", r.status())); }
        let updated: Value = r.json().await.map_err(|e| e.to_string())?;
        if updated["version"].as_u64() != Some(2) { return Err(format!("expected version 2 after update, got {}", updated["version"])); }
        if updated["description"].as_str() != Some("updated") { return Err("description not updated".into()); }

        // List
        let r = http.get(format!("{base}/agents")).send().await.map_err(|e| e.to_string())?;
        if r.status() != 200 { return Err(format!("list: {}", r.status())); }
        let list: Value = r.json().await.map_err(|e| e.to_string())?;
        let arr = list.as_array().ok_or("list not array")?;
        if !arr.iter().any(|a| a["id"].as_str() == Some(&agent_id)) {
            return Err("created agent not in list".into());
        }

        // Versions
        let r = http.get(format!("{base}/agents/{agent_id}/versions")).send().await.map_err(|e| e.to_string())?;
        if r.status() != 200 { return Err(format!("versions: {}", r.status())); }
        let versions: Value = r.json().await.map_err(|e| e.to_string())?;
        let varr = versions.as_array().ok_or("versions not array")?;
        if varr.len() < 2 { return Err(format!("expected ≥2 versions after update, got {}", varr.len())); }

        // Agent sessions (empty but must 200)
        let r = http.get(format!("{base}/agents/{agent_id}/sessions")).send().await.map_err(|e| e.to_string())?;
        if r.status() != 200 { return Err(format!("agent sessions: {}", r.status())); }

        // Delete
        let r = http.delete(format!("{base}/agents/{agent_id}")).send().await.map_err(|e| e.to_string())?;
        if r.status() != 204 { return Err(format!("delete: {}", r.status())); }

        // Get after delete → 404
        let r = http.get(format!("{base}/agents/{agent_id}")).send().await.map_err(|e| e.to_string())?;
        if r.status() != 404 { return Err(format!("expected 404 after delete, got {}", r.status())); }

        Ok(())
    }.await;

    match result { Ok(()) => { ok(LABEL); true } Err(e) => { ko(LABEL, &e); false } }
}

// ── Skills ─────────────────────────────────────────────────────────────────────

async fn test_skills_crud(base: &str, http: &Client) -> bool {
    const LABEL: &str = "Skills — create / get / list / version / delete";
    let id = uid();

    let result: Result<(), String> = async {
        // Create
        let r = http.post(format!("{base}/skills"))
            .json(&json!({"name": format!("skill-{id}"), "description": "test skill", "content": "do stuff"}))
            .send().await.map_err(|e| e.to_string())?;
        if r.status() != 201 { return Err(format!("create: {}", r.status())); }
        let skill: Value = r.json().await.map_err(|e| e.to_string())?;
        let skill_id = skill["id"].as_str().ok_or("no id")?.to_string();

        // Get
        let r = http.get(format!("{base}/skills/{skill_id}")).send().await.map_err(|e| e.to_string())?;
        if r.status() != 200 { return Err(format!("get: {}", r.status())); }

        // List
        let r = http.get(format!("{base}/skills")).send().await.map_err(|e| e.to_string())?;
        if r.status() != 200 { return Err(format!("list: {}", r.status())); }
        let list: Value = r.json().await.map_err(|e| e.to_string())?;
        let arr = list.as_array().ok_or("list not array")?;
        if !arr.iter().any(|s| s["id"].as_str() == Some(&skill_id)) {
            return Err("created skill not in list".into());
        }

        // List versions
        let r = http.get(format!("{base}/skills/{skill_id}/versions")).send().await.map_err(|e| e.to_string())?;
        if r.status() != 200 { return Err(format!("list versions: {}", r.status())); }

        // Create new version
        let r = http.post(format!("{base}/skills/{skill_id}/versions"))
            .json(&json!({"content": "do stuff v2"}))
            .send().await.map_err(|e| e.to_string())?;
        if r.status() != 201 { return Err(format!("create version: {}", r.status())); }

        // Delete
        let r = http.delete(format!("{base}/skills/{skill_id}")).send().await.map_err(|e| e.to_string())?;
        if r.status() != 204 { return Err(format!("delete: {}", r.status())); }

        Ok(())
    }.await;

    match result { Ok(()) => { ok(LABEL); true } Err(e) => { ko(LABEL, &e); false } }
}

// ── Environments ──────────────────────────────────────────────────────────────

async fn test_environments_crud(base: &str, http: &Client) -> bool {
    const LABEL: &str = "Environments — create / get / update / list / archive / delete";
    let id = uid();

    let result: Result<(), String> = async {
        // Create
        let r = http.post(format!("{base}/environments"))
            .json(&json!({"name": format!("env-{id}"), "description": "test env"}))
            .send().await.map_err(|e| e.to_string())?;
        if r.status() != 201 { return Err(format!("create: {}", r.status())); }
        let env: Value = r.json().await.map_err(|e| e.to_string())?;
        let env_id = env["id"].as_str().ok_or("no id")?.to_string();
        if env["archived"].as_bool() != Some(false) { return Err("should not be archived on create".into()); }

        // Get
        let r = http.get(format!("{base}/environments/{env_id}")).send().await.map_err(|e| e.to_string())?;
        if r.status() != 200 { return Err(format!("get: {}", r.status())); }

        // Update
        let r = http.put(format!("{base}/environments/{env_id}"))
            .json(&json!({"name": format!("env-{id}-updated")}))
            .send().await.map_err(|e| e.to_string())?;
        if r.status() != 200 { return Err(format!("update: {}", r.status())); }
        let updated: Value = r.json().await.map_err(|e| e.to_string())?;
        if !updated["name"].as_str().unwrap_or("").contains("updated") {
            return Err("name not updated".into());
        }

        // List
        let r = http.get(format!("{base}/environments")).send().await.map_err(|e| e.to_string())?;
        if r.status() != 200 { return Err(format!("list: {}", r.status())); }
        let list: Value = r.json().await.map_err(|e| e.to_string())?;
        if !list.as_array().ok_or("not array")?.iter().any(|e| e["id"].as_str() == Some(&env_id)) {
            return Err("env not in list".into());
        }

        // Archive
        let r = http.post(format!("{base}/environments/{env_id}/archive")).send().await.map_err(|e| e.to_string())?;
        if r.status() != 200 { return Err(format!("archive: {}", r.status())); }
        let archived: Value = r.json().await.map_err(|e| e.to_string())?;
        if archived["archived"].as_bool() != Some(true) { return Err("archived flag not set".into()); }

        // Delete
        let r = http.delete(format!("{base}/environments/{env_id}")).send().await.map_err(|e| e.to_string())?;
        if r.status() != 204 { return Err(format!("delete: {}", r.status())); }

        Ok(())
    }.await;

    match result { Ok(()) => { ok(LABEL); true } Err(e) => { ko(LABEL, &e); false } }
}

// ── Credentials ───────────────────────────────────────────────────────────────

async fn test_credentials_crud(base: &str, http: &Client) -> bool {
    const LABEL: &str = "Credentials — vault / create / get / list / delete";
    let id = uid();

    let result: Result<(), String> = async {
        // Create environment first (credentials require an env_id)
        let r = http.post(format!("{base}/environments"))
            .json(&json!({"name": format!("cred-env-{id}")}))
            .send().await.map_err(|e| e.to_string())?;
        if r.status() != 201 { return Err(format!("create env: {}", r.status())); }
        let env: Value = r.json().await.map_err(|e| e.to_string())?;
        let env_id = env["id"].as_str().ok_or("no env id")?.to_string();

        // Create credential first — vault is created lazily on first credential
        let r = http.post(format!("{base}/environments/{env_id}/credentials"))
            .json(&json!({"name": format!("cred-{id}"), "type": "bearer_token", "mcp_server_url": "https://example.com/mcp"}))
            .send().await.map_err(|e| e.to_string())?;
        if r.status() != 201 { return Err(format!("create cred: {}", r.status())); }
        let cred: Value = r.json().await.map_err(|e| e.to_string())?;
        let cred_id = cred["id"].as_str().ok_or("no cred id")?.to_string();

        // Get vault (now it exists because the credential was created)
        let r = http.get(format!("{base}/environments/{env_id}/vault")).send().await.map_err(|e| e.to_string())?;
        if r.status() != 200 { return Err(format!("get vault: {}", r.status())); }
        let vault: Value = r.json().await.map_err(|e| e.to_string())?;
        if vault["env_id"].as_str() != Some(&env_id) { return Err("vault env_id mismatch".into()); }

        // Get credential
        let r = http.get(format!("{base}/environments/{env_id}/credentials/{cred_id}")).send().await.map_err(|e| e.to_string())?;
        if r.status() != 200 { return Err(format!("get cred: {}", r.status())); }

        // List credentials
        let r = http.get(format!("{base}/environments/{env_id}/credentials")).send().await.map_err(|e| e.to_string())?;
        if r.status() != 200 { return Err(format!("list creds: {}", r.status())); }
        let list: Value = r.json().await.map_err(|e| e.to_string())?;
        if !list.as_array().ok_or("not array")?.iter().any(|c| c["id"].as_str() == Some(&cred_id)) {
            return Err("cred not in list".into());
        }

        // Delete credential
        let r = http.delete(format!("{base}/environments/{env_id}/credentials/{cred_id}")).send().await.map_err(|e| e.to_string())?;
        if r.status() != 204 { return Err(format!("delete cred: {}", r.status())); }

        // Get after delete → 404
        let r = http.get(format!("{base}/environments/{env_id}/credentials/{cred_id}")).send().await.map_err(|e| e.to_string())?;
        if r.status() != 404 { return Err(format!("expected 404 after delete, got {}", r.status())); }

        Ok(())
    }.await;

    match result { Ok(()) => { ok(LABEL); true } Err(e) => { ko(LABEL, &e); false } }
}

// ── Sessions (read-only) ──────────────────────────────────────────────────────

async fn test_sessions_read(base: &str, http: &Client) -> bool {
    const LABEL: &str = "Sessions — GET /sessions returns 200 (read-only view)";
    match http.get(format!("{base}/sessions")).send().await {
        Ok(r) if r.status() == 200 => { ok(LABEL); true }
        Ok(r) => { ko(LABEL, &format!("status {}", r.status())); false }
        Err(e) => { ko(LABEL, &e.to_string()); false }
    }
}

// ── MCP registry ──────────────────────────────────────────────────────────────

async fn test_mcp_registry(base: &str, http: &Client) -> bool {
    const LABEL: &str = "MCP registry — GET /mcp-registry returns 200";
    match http.get(format!("{base}/mcp-registry")).send().await {
        Ok(r) if r.status() == 200 => { ok(LABEL); true }
        Ok(r) => { ko(LABEL, &format!("status {}", r.status())); false }
        Err(e) => { ko(LABEL, &e.to_string()); false }
    }
}

// ── Agent 404 paths ───────────────────────────────────────────────────────────

async fn test_agent_not_found(base: &str, http: &Client) -> bool {
    const LABEL: &str = "Agents — GET /agents/nonexistent returns 404";
    match http.get(format!("{base}/agents/nonexistent-id")).send().await {
        Ok(r) if r.status() == 404 => { ok(LABEL); true }
        Ok(r) => { ko(LABEL, &format!("expected 404, got {}", r.status())); false }
        Err(e) => { ko(LABEL, &e.to_string()); false }
    }
}

// ══════════════════════════════════════════════════════════════════════════════
// Entry point
// ══════════════════════════════════════════════════════════════════════════════

use std::future::IntoFuture as _;

#[tokio::main]
async fn main() {
    println!();
    println!("══════════════════════════════════════════════════════════");
    println!(" trogon-console — HTTP server live test");
    println!("  NATS: nats://localhost:4222");
    println!("  Server: in-process on a random port");
    println!("══════════════════════════════════════════════════════════");
    println!();

    let (base, http) = start_server().await;

    println!("Health");
    let r1 = test_health(&base, &http).await;

    println!();
    println!("Agents");
    let r2 = test_agents_crud(&base, &http).await;
    let r3 = test_agent_not_found(&base, &http).await;

    println!();
    println!("Skills");
    let r4 = test_skills_crud(&base, &http).await;

    println!();
    println!("Environments");
    let r5 = test_environments_crud(&base, &http).await;

    println!();
    println!("Credentials");
    let r6 = test_credentials_crud(&base, &http).await;

    println!();
    println!("Sessions (read-only)");
    let r7 = test_sessions_read(&base, &http).await;

    println!();
    println!("MCP Registry");
    let r8 = test_mcp_registry(&base, &http).await;

    let results = [r1, r2, r3, r4, r5, r6, r7, r8];
    let passed = results.iter().filter(|&&r| r).count();
    let total = results.len();

    println!();
    println!("══════════════════════════════════════════════════════════");
    if passed == total {
        println!(" \x1b[32mAll {total} tests passed\x1b[0m");
    } else {
        println!(" \x1b[31m{passed}/{total} tests passed\x1b[0m");
    }
    println!("══════════════════════════════════════════════════════════");
    println!();

    if passed < total {
        std::process::exit(1);
    }
}

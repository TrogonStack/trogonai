//! End-to-end contract tests: verifies that the trogon-console REST API writes
//! NATS KV entries in the exact format that the trogon-agent loaders (SkillLoader,
//! AgentLoader) expect to read.
//!
//! These tests do NOT import trogon-agent. Instead they read the KV buckets
//! directly with async-nats and assert that the key/value structure matches the
//! documented contract, guaranteeing interoperability without a compile-time
//! dependency on the agent crate.
//!
//! Contract being verified:
//!
//! CONSOLE_SKILLS bucket
//!   key  : {skill_id}
//!   value: JSON with at least { "name": string, "latest_version": string }
//!
//! CONSOLE_SKILL_VERSIONS bucket
//!   key  : {skill_id}.{version}
//!   value: JSON with at least { "content": string }
//!
//! CONSOLE_AGENTS bucket
//!   key  : {agent_id}
//!   value: JSON with at least { "skill_ids": [string] }

use async_nats::jetstream;
use bytes::Bytes;
use reqwest::Client;
use serde_json::Value;
use testcontainers_modules::nats::Nats;
use testcontainers_modules::testcontainers::{ImageExt, runners::AsyncRunner};
use tokio::net::TcpListener;
use trogon_console::{
    server::{AppState, build_router},
    store::{
        agents::AgentStore, credentials::CredentialStore, environments::EnvironmentStore,
        sessions::SessionReader, skills::SkillStore,
    },
};

use std::sync::Arc;

// ── Harness ───────────────────────────────────────────────────────────────────

struct TestEnv {
    base_url: String,
    client: Client,
    js: jetstream::Context,
    _nats: Box<dyn std::any::Any>,
}

async fn start() -> TestEnv {
    let container = Nats::default()
        .with_cmd(["--jetstream"])
        .start()
        .await
        .expect("NATS container start");
    let port = container.get_host_port_ipv4(4222).await.expect("port");
    let nats = async_nats::connect(format!("nats://127.0.0.1:{port}"))
        .await
        .expect("NATS connect");
    let js = jetstream::new(nats);

    let agents = Arc::new(AgentStore::open(&js).await.expect("AgentStore"));
    let skills = Arc::new(SkillStore::open(&js).await.expect("SkillStore"));
    let environments = Arc::new(EnvironmentStore::open(&js).await.expect("EnvironmentStore"));
    let credentials = Arc::new(CredentialStore::open(&js).await.expect("CredentialStore"));
    let sessions = Arc::new(SessionReader::open(&js).await.expect("SessionReader"));

    let state = Arc::new(AppState {
        agents,
        skills,
        environments,
        credentials,
        sessions,
    });
    let app = build_router(state);

    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("addr");
    tokio::spawn(async move { axum::serve(listener, app).await.expect("server") });

    TestEnv {
        base_url: format!("http://{addr}"),
        client: Client::new(),
        js,
        _nats: Box::new(container),
    }
}

async fn kv_get_json(js: &jetstream::Context, bucket: &str, key: &str) -> Value {
    let kv = js
        .get_key_value(bucket)
        .await
        .unwrap_or_else(|e| panic!("get_key_value({bucket}): {e}"));
    let bytes = kv
        .get(key)
        .await
        .unwrap_or_else(|e| panic!("kv.get({key}): {e}"))
        .unwrap_or_else(|| panic!("key {key} not found in {bucket}"));
    serde_json::from_slice(&bytes)
        .unwrap_or_else(|e| panic!("JSON parse error for {bucket}/{key}: {e}"))
}

// ── SkillLoader contract: CONSOLE_SKILLS and CONSOLE_SKILL_VERSIONS ───────────

/// After POST /skills, CONSOLE_SKILLS contains { name, latest_version } and
/// CONSOLE_SKILL_VERSIONS contains { content } at key {skill_id}.{version}.
#[tokio::test]
async fn skill_written_by_console_has_correct_kv_format_for_skill_loader() {
    let env = start().await;

    let resp = env
        .client
        .post(format!("{}/skills", env.base_url))
        .json(&serde_json::json!({
            "name": "Onboarding Guide",
            "description": "Explains how to onboard new users",
            "content": "Always greet new users and ask for their name before proceeding."
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 201);
    let created: Value = resp.json().await.unwrap();
    let skill_id = created["id"].as_str().unwrap();

    // CONSOLE_SKILLS entry must have name + latest_version (used by SkillLoader)
    let meta = kv_get_json(&env.js, "CONSOLE_SKILLS", skill_id).await;
    assert_eq!(meta["name"], "Onboarding Guide", "name mismatch: {meta}");
    let version = meta["latest_version"]
        .as_str()
        .unwrap_or_else(|| panic!("latest_version missing: {meta}"));
    assert!(!version.is_empty(), "latest_version must not be empty");

    // CONSOLE_SKILL_VERSIONS entry must have content at key {skill_id}.{version}
    let ver_key = format!("{skill_id}.{version}");
    let ver = kv_get_json(&env.js, "CONSOLE_SKILL_VERSIONS", &ver_key).await;
    assert_eq!(
        ver["content"], "Always greet new users and ask for their name before proceeding.",
        "content mismatch: {ver}"
    );
}

/// After POST /skills/{id}/versions, CONSOLE_SKILLS latest_version is updated
/// and CONSOLE_SKILL_VERSIONS contains the new content.
#[tokio::test]
async fn adding_skill_version_updates_latest_version_in_kv() {
    let env = start().await;

    let resp = env
        .client
        .post(format!("{}/skills", env.base_url))
        .json(&serde_json::json!({
            "name": "Auth Policy",
            "description": "",
            "content": "v1 content"
        }))
        .send()
        .await
        .unwrap();
    let created: Value = resp.json().await.unwrap();
    let skill_id = created["id"].as_str().unwrap();
    let v1 = created["latest_version"].as_str().unwrap().to_string();

    // Add a new version
    let resp = env
        .client
        .post(format!("{}/skills/{skill_id}/versions", env.base_url))
        .json(&serde_json::json!({ "content": "v2 content" }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 201);
    let v2_resp: Value = resp.json().await.unwrap();
    let v2 = v2_resp["version"].as_str().unwrap().to_string();

    // latest_version in the skills bucket must point to v2
    let meta = kv_get_json(&env.js, "CONSOLE_SKILLS", skill_id).await;
    let stored_latest = meta["latest_version"].as_str().unwrap();
    assert_eq!(stored_latest, v2, "latest_version should be updated to v2");

    // v2 version entry must exist with new content
    let ver = kv_get_json(
        &env.js,
        "CONSOLE_SKILL_VERSIONS",
        &format!("{skill_id}.{v2}"),
    )
    .await;
    assert_eq!(ver["content"], "v2 content");

    // v1 version entry may still exist (no cleanup expected) or may be overwritten
    // on same-day runs — either is fine; we only assert v2 is readable.
    let _ = v1; // suppress unused warning
}

// ── AgentLoader contract: CONSOLE_AGENTS ─────────────────────────────────────

/// After POST /agents with skill_ids, CONSOLE_AGENTS contains { skill_ids: [...] }
/// (the field AgentLoader reads).
#[tokio::test]
async fn agent_written_by_console_has_skill_ids_in_kv_for_agent_loader() {
    let env = start().await;

    let resp = env
        .client
        .post(format!("{}/agents", env.base_url))
        .json(&serde_json::json!({
            "name": "Skilled Agent",
            "description": "Agent with skills",
            "model": { "id": "claude-sonnet-4-6" },
            "system_prompt": "You are a skilled agent.",
            "skill_ids": ["skill_abc", "skill_xyz"]
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 201);
    let created: Value = resp.json().await.unwrap();
    let agent_id = created["id"].as_str().unwrap();

    let entry = kv_get_json(&env.js, "CONSOLE_AGENTS", agent_id).await;
    let ids: Vec<&str> = entry["skill_ids"]
        .as_array()
        .unwrap_or_else(|| panic!("skill_ids missing or not an array: {entry}"))
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();

    assert_eq!(ids, vec!["skill_abc", "skill_xyz"]);
}

/// An agent created without skill_ids has an empty array in CONSOLE_AGENTS.
#[tokio::test]
async fn agent_without_skills_has_empty_skill_ids_in_kv() {
    let env = start().await;

    let resp = env
        .client
        .post(format!("{}/agents", env.base_url))
        .json(&serde_json::json!({
            "name": "Bare Agent",
            "description": "",
            "model": { "id": "claude-sonnet-4-6" },
            "system_prompt": "."
        }))
        .send()
        .await
        .unwrap();
    let created: Value = resp.json().await.unwrap();
    let agent_id = created["id"].as_str().unwrap();

    let entry = kv_get_json(&env.js, "CONSOLE_AGENTS", agent_id).await;
    let ids = entry["skill_ids"]
        .as_array()
        .unwrap_or_else(|| panic!("skill_ids missing: {entry}"));
    assert!(
        ids.is_empty(),
        "bare agent should have empty skill_ids: {ids:?}"
    );
}

/// After PUT /agents/{id} updating skill_ids, CONSOLE_AGENTS reflects the new list.
#[tokio::test]
async fn updating_agent_skill_ids_is_reflected_in_kv() {
    let env = start().await;

    let resp = env
        .client
        .post(format!("{}/agents", env.base_url))
        .json(&serde_json::json!({
            "name": "Evolving Agent",
            "description": "",
            "model": { "id": "claude-sonnet-4-6" },
            "system_prompt": ".",
            "skill_ids": ["skill_v1"]
        }))
        .send()
        .await
        .unwrap();
    let created: Value = resp.json().await.unwrap();
    let agent_id = created["id"].as_str().unwrap();

    env.client
        .put(format!("{}/agents/{agent_id}", env.base_url))
        .json(&serde_json::json!({ "skill_ids": ["skill_v2", "skill_v3"] }))
        .send()
        .await
        .unwrap();

    let entry = kv_get_json(&env.js, "CONSOLE_AGENTS", agent_id).await;
    let ids: Vec<&str> = entry["skill_ids"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert_eq!(ids, vec!["skill_v2", "skill_v3"]);
}

// ── Full round-trip ───────────────────────────────────────────────────────────

/// Console creates skill + agent → KV contains everything needed for agent
/// runtime to assemble the injected system prompt.
#[tokio::test]
async fn full_round_trip_kv_structure_is_complete() {
    let env = start().await;

    // Create skill
    let skill_resp: Value = env
        .client
        .post(format!("{}/skills", env.base_url))
        .json(&serde_json::json!({
            "name": "Security Policy",
            "description": "",
            "content": "Never expose secrets. Always validate inputs."
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let skill_id = skill_resp["id"].as_str().unwrap();

    // Create agent referencing that skill
    let agent_resp: Value = env
        .client
        .post(format!("{}/agents", env.base_url))
        .json(&serde_json::json!({
            "name": "Security Agent",
            "description": "",
            "model": { "id": "claude-sonnet-4-6" },
            "system_prompt": "You are a security agent.",
            "skill_ids": [skill_id]
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let agent_id = agent_resp["id"].as_str().unwrap();

    // Step 1: AgentLoader would read skill_ids from CONSOLE_AGENTS
    let agent_entry = kv_get_json(&env.js, "CONSOLE_AGENTS", agent_id).await;
    let skill_ids: Vec<&str> = agent_entry["skill_ids"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert_eq!(
        skill_ids,
        vec![skill_id],
        "agent must reference the created skill"
    );

    // Step 2: SkillLoader would read latest_version from CONSOLE_SKILLS
    let skill_meta = kv_get_json(&env.js, "CONSOLE_SKILLS", skill_id).await;
    let latest_version = skill_meta["latest_version"]
        .as_str()
        .unwrap_or_else(|| panic!("latest_version missing"));
    let skill_name = skill_meta["name"].as_str().unwrap();
    assert_eq!(skill_name, "Security Policy");

    // Step 3: SkillLoader would read content from CONSOLE_SKILL_VERSIONS
    let ver_key = format!("{skill_id}.{latest_version}");
    let ver_entry = kv_get_json(&env.js, "CONSOLE_SKILL_VERSIONS", &ver_key).await;
    let content = ver_entry["content"].as_str().unwrap();
    assert_eq!(content, "Never expose secrets. Always validate inputs.");
}

// ── Session read contract: agent writes, console reads ────────────────────────
//
// The console's SessionReader deserializes RawSession (written by trogon-agent)
// and derives status, message_count, and token totals. These tests write the
// raw format directly into the SESSIONS KV bucket and verify the console API
// returns the correct derived fields.

async fn write_raw_session(js: &jetstream::Context, tenant_id: &str, raw: serde_json::Value) {
    let id = raw["id"].as_str().unwrap().to_string();
    let kv = js.get_key_value("SESSIONS").await.expect("SESSIONS bucket");
    kv.put(
        format!("{tenant_id}.{id}"),
        Bytes::from(serde_json::to_vec(&raw).unwrap()),
    )
    .await
    .expect("put raw session");
}

/// Full field mapping: token sums, message_count, duration_ms, agent_id, and
/// idle status (last message is "assistant") are all derived correctly.
#[tokio::test]
async fn session_written_by_agent_is_readable_by_console_api() {
    let env = start().await;

    write_raw_session(&env.js, "tenant_abc", serde_json::json!({
        "id": "sess_001",
        "tenant_id": "tenant_abc",
        "name": "Test Session",
        "model": "claude-sonnet-4-6",
        "messages": [
            { "role": "user",      "usage": { "input_tokens": 100, "output_tokens": 0,   "cache_creation_input_tokens": 50, "cache_read_input_tokens": 0  } },
            { "role": "assistant", "usage": { "input_tokens": 0,   "output_tokens": 200, "cache_creation_input_tokens": 0,  "cache_read_input_tokens": 25 } }
        ],
        "duration_ms": 1500,
        "agent_id": "agent_xyz",
        "created_at": "1000",
        "updated_at": "2000"
    })).await;

    let resp = env
        .client
        .get(format!("{}/sessions/tenant_abc/sess_001", env.base_url))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 200);
    let s: Value = resp.json().await.unwrap();

    assert_eq!(s["id"], "sess_001");
    assert_eq!(s["tenant_id"], "tenant_abc");
    assert_eq!(s["model"], "claude-sonnet-4-6");
    assert_eq!(s["message_count"], 2);
    assert_eq!(s["input_tokens"], 100);
    assert_eq!(s["output_tokens"], 200);
    assert_eq!(s["cache_write_tokens"], 50);
    assert_eq!(s["cache_read_tokens"], 25);
    assert_eq!(s["duration_ms"], 1500);
    assert_eq!(s["agent_id"], "agent_xyz");
    assert_eq!(s["status"], "idle"); // last message is "assistant"
}

/// A session whose last message is from "user" has status "running".
#[tokio::test]
async fn session_with_last_user_message_has_running_status() {
    let env = start().await;

    write_raw_session(
        &env.js,
        "tenant_run",
        serde_json::json!({
            "id": "sess_running",
            "tenant_id": "tenant_run",
            "name": "Running Session",
            "messages": [
                { "role": "assistant", "usage": null },
                { "role": "user",      "usage": null }
            ],
            "created_at": "1", "updated_at": "2"
        }),
    )
    .await;

    let s: Value = env
        .client
        .get(format!("{}/sessions/tenant_run/sess_running", env.base_url))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    assert_eq!(s["status"], "running");
    assert_eq!(s["message_count"], 2);
    assert_eq!(s["input_tokens"], 0);
}

/// A session with no messages has status "idle" and zero counts.
#[tokio::test]
async fn empty_session_has_idle_status_and_zero_counts() {
    let env = start().await;

    write_raw_session(
        &env.js,
        "tenant_empty",
        serde_json::json!({
            "id": "sess_empty",
            "tenant_id": "tenant_empty",
            "name": "Empty",
            "messages": [],
            "created_at": "1", "updated_at": "2"
        }),
    )
    .await;

    let s: Value = env
        .client
        .get(format!("{}/sessions/tenant_empty/sess_empty", env.base_url))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    assert_eq!(s["status"], "idle");
    assert_eq!(s["message_count"], 0);
    assert_eq!(s["input_tokens"], 0);
    assert_eq!(s["output_tokens"], 0);
}

// ── Rollback contract: CONSOLE_AGENTS after rollback ─────────────────────────

/// After POST /agents/{id}/rollback/{version}, CONSOLE_AGENTS reflects the
/// system_prompt from the target version snapshot.
#[tokio::test]
async fn rollback_writes_correct_system_prompt_to_kv() {
    let env = start().await;

    // Create agent with v1 system_prompt
    let created: Value = env
        .client
        .post(format!("{}/agents", env.base_url))
        .json(&serde_json::json!({
            "name": "Rollback Agent",
            "description": "",
            "model": { "id": "claude-opus-4-7" },
            "system_prompt": "v1 prompt"
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let agent_id = created["id"].as_str().unwrap();

    // Update to v2 with bad prompt
    env.client
        .put(format!("{}/agents/{agent_id}", env.base_url))
        .json(&serde_json::json!({ "system_prompt": "bad prompt" }))
        .send()
        .await
        .unwrap();

    // Rollback to v1
    let rolled: Value = env
        .client
        .post(format!("{}/agents/{agent_id}/rollback/1", env.base_url))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(rolled["system_prompt"], "v1 prompt");
    assert_eq!(rolled["version"], 3);

    // Verify CONSOLE_AGENTS KV reflects the rollback
    let entry = kv_get_json(&env.js, "CONSOLE_AGENTS", agent_id).await;
    assert_eq!(
        entry["system_prompt"], "v1 prompt",
        "CONSOLE_AGENTS must reflect rolled-back system_prompt: {entry}"
    );
    assert_eq!(entry["version"], 3);
}

/// GET /sessions lists all sessions regardless of tenant.
#[tokio::test]
async fn list_sessions_returns_all_agent_written_sessions() {
    let env = start().await;

    for (tenant, id) in [("t1", "s1"), ("t2", "s2")] {
        write_raw_session(
            &env.js,
            tenant,
            serde_json::json!({
                "id": id,
                "tenant_id": tenant,
                "name": format!("Session {id}"),
                "messages": [],
                "created_at": "1", "updated_at": "1"
            }),
        )
        .await;
    }

    let sessions: Value = env
        .client
        .get(format!("{}/sessions", env.base_url))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    assert_eq!(sessions.as_array().unwrap().len(), 2);
}

#[tokio::test]
async fn list_versions_for_deleted_agent_returns_404() {
    let env = start().await;

    let agent: Value = env
        .client
        .post(format!("{}/agents", env.base_url))
        .json(&serde_json::json!({
            "name": "Agent", "description": "", "model": { "id": "m" }, "system_prompt": "s"
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let id = agent["id"].as_str().unwrap();

    env.client
        .delete(format!("{}/agents/{id}", env.base_url))
        .send()
        .await
        .unwrap();

    let status = env
        .client
        .get(format!("{}/agents/{id}/versions", env.base_url))
        .send()
        .await
        .unwrap()
        .status();
    assert_eq!(status, 404);
}

#[tokio::test]
async fn get_version_for_deleted_agent_returns_404() {
    let env = start().await;

    let agent: Value = env
        .client
        .post(format!("{}/agents", env.base_url))
        .json(&serde_json::json!({
            "name": "Agent", "description": "", "model": { "id": "m" }, "system_prompt": "s"
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let id = agent["id"].as_str().unwrap();

    env.client
        .delete(format!("{}/agents/{id}", env.base_url))
        .send()
        .await
        .unwrap();

    let status = env
        .client
        .get(format!("{}/agents/{id}/versions/1", env.base_url))
        .send()
        .await
        .unwrap()
        .status();
    assert_eq!(status, 404);
}

#[tokio::test]
async fn list_versions_returns_summary_for_each_version() {
    let env = start().await;

    let agent: Value = env
        .client
        .post(format!("{}/agents", env.base_url))
        .json(&serde_json::json!({
            "name": "Agent", "description": "", "model": { "id": "claude-3" }, "system_prompt": "v1"
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let id = agent["id"].as_str().unwrap();

    env.client
        .put(format!("{}/agents/{id}", env.base_url))
        .json(&serde_json::json!({ "system_prompt": "v2" }))
        .send()
        .await
        .unwrap();

    let versions: Value = env
        .client
        .get(format!("{}/agents/{id}/versions", env.base_url))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    let arr = versions.as_array().unwrap();
    assert_eq!(arr.len(), 2);
    assert_eq!(arr[0]["version"], 1);
    assert_eq!(arr[0]["model_id"], "claude-3");
    assert_eq!(arr[1]["version"], 2);
    assert_eq!(arr[1]["model_id"], "claude-3");
}

#[tokio::test]
async fn get_version_returns_full_definition_with_correct_system_prompt() {
    let env = start().await;

    let agent: Value = env
        .client
        .post(format!("{}/agents", env.base_url))
        .json(&serde_json::json!({
            "name": "Agent", "description": "", "model": { "id": "m" }, "system_prompt": "original"
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let id = agent["id"].as_str().unwrap();

    env.client
        .put(format!("{}/agents/{id}", env.base_url))
        .json(&serde_json::json!({ "system_prompt": "updated" }))
        .send()
        .await
        .unwrap();

    let snap: Value = env
        .client
        .get(format!("{}/agents/{id}/versions/1", env.base_url))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    assert_eq!(snap["version"], 1);
    assert_eq!(snap["system_prompt"], "original");

    let snap2: Value = env
        .client
        .get(format!("{}/agents/{id}/versions/2", env.base_url))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    assert_eq!(snap2["version"], 2);
    assert_eq!(snap2["system_prompt"], "updated");
}

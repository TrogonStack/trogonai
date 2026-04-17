//! Integration tests for the trogon-console REST API.
//!
//! Spins up a real NATS container, wires real KV-backed stores into an Axum
//! server, and exercises every route via reqwest.

use std::sync::Arc;

use async_nats::jetstream;
use bytes::Bytes;
use reqwest::Client;
use serde_json::{Value, json};
use testcontainers_modules::nats::Nats;
use testcontainers_modules::testcontainers::{ImageExt, runners::AsyncRunner};
use tokio::net::TcpListener;
use trogon_console::{
    server::{AppState, build_router},
    store::{
        agents::AgentStore,
        credentials::CredentialStore,
        environments::EnvironmentStore,
        sessions::SessionReader,
        skills::SkillStore,
    },
};

// ── Test harness ──────────────────────────────────────────────────────────────

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

    let state = Arc::new(AppState { agents, skills, environments, credentials, sessions });
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

// ── Health ────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn health_returns_ok() {
    let env = start().await;
    let resp = env.client.get(format!("{}/-/health", env.base_url))
        .send().await.unwrap();
    assert_eq!(resp.status().as_u16(), 200);
}

// ── Agent CRUD ────────────────────────────────────────────────────────────────

#[tokio::test]
async fn agent_create_list_get_update_delete() {
    let env = start().await;
    let base = &env.base_url;

    // Create
    let body = json!({
        "name": "TestAgent",
        "description": "Integration test agent",
        "model": { "id": "claude-sonnet-4-6" },
        "system_prompt": "You are a test agent.",
        "skill_ids": ["skill_abc"]
    });
    let resp = env.client.post(format!("{base}/agents"))
        .json(&body).send().await.unwrap();
    assert_eq!(resp.status().as_u16(), 201);
    let created: Value = resp.json().await.unwrap();
    let agent_id = created["id"].as_str().unwrap().to_string();
    assert!(agent_id.starts_with("agent_"), "id should start with agent_: {agent_id}");
    assert_eq!(created["name"], "TestAgent");
    assert_eq!(created["version"], 1);
    assert_eq!(created["skill_ids"], json!(["skill_abc"]));

    // List — agent appears
    let resp = env.client.get(format!("{base}/agents")).send().await.unwrap();
    assert_eq!(resp.status().as_u16(), 200);
    let list: Value = resp.json().await.unwrap();
    let ids: Vec<&str> = list.as_array().unwrap().iter()
        .map(|a| a["id"].as_str().unwrap()).collect();
    assert!(ids.contains(&agent_id.as_str()), "agent not in list");

    // Get
    let resp = env.client.get(format!("{base}/agents/{agent_id}")).send().await.unwrap();
    assert_eq!(resp.status().as_u16(), 200);
    let got: Value = resp.json().await.unwrap();
    assert_eq!(got["id"], agent_id);

    // Get non-existent → 404
    let resp = env.client.get(format!("{base}/agents/agent_nonexistent")).send().await.unwrap();
    assert_eq!(resp.status().as_u16(), 404);

    // Update — change name and add a skill_id, bumps version
    let update = json!({
        "name": "UpdatedAgent",
        "skill_ids": ["skill_abc", "skill_xyz"]
    });
    let resp = env.client.put(format!("{base}/agents/{agent_id}"))
        .json(&update).send().await.unwrap();
    assert_eq!(resp.status().as_u16(), 200);
    let updated: Value = resp.json().await.unwrap();
    assert_eq!(updated["name"], "UpdatedAgent");
    assert_eq!(updated["version"], 2);
    assert_eq!(updated["skill_ids"], json!(["skill_abc", "skill_xyz"]));

    // List versions — two entries (v1 and v2)
    let resp = env.client.get(format!("{base}/agents/{agent_id}/versions")).send().await.unwrap();
    assert_eq!(resp.status().as_u16(), 200);
    let versions: Value = resp.json().await.unwrap();
    assert_eq!(versions.as_array().unwrap().len(), 2, "expected 2 versions");

    // Delete
    let resp = env.client.delete(format!("{base}/agents/{agent_id}")).send().await.unwrap();
    assert_eq!(resp.status().as_u16(), 204);

    // List — agent no longer present (NATS KV delete marks as tombstone; keys() skips tombstones)
    let resp = env.client.get(format!("{base}/agents")).send().await.unwrap();
    let list: Value = resp.json().await.unwrap();
    let ids_after: Vec<&str> = list.as_array().unwrap().iter()
        .filter_map(|a| a["id"].as_str()).collect();
    assert!(!ids_after.contains(&agent_id.as_str()), "deleted agent still in list");
}

// ── Skill CRUD ────────────────────────────────────────────────────────────────

#[tokio::test]
async fn skill_create_list_get_add_version() {
    let env = start().await;
    let base = &env.base_url;

    // Create
    let body = json!({
        "name": "pdf-extractor",
        "description": "Extracts text from PDFs",
        "content": "## PDF Extractor\nUse this skill to extract text."
    });
    let resp = env.client.post(format!("{base}/skills"))
        .json(&body).send().await.unwrap();
    assert_eq!(resp.status().as_u16(), 201);
    let created: Value = resp.json().await.unwrap();
    let skill_id = created["id"].as_str().unwrap().to_string();
    assert!(skill_id.starts_with("skill_"), "id should start with skill_: {skill_id}");
    assert_eq!(created["name"], "pdf-extractor");
    assert_eq!(created["provider"], "custom");

    let v1 = created["latest_version"].as_str().unwrap().to_string();
    assert_eq!(v1.len(), 8, "version must be YYYYMMDD: {v1}");

    // List
    let resp = env.client.get(format!("{base}/skills")).send().await.unwrap();
    assert_eq!(resp.status().as_u16(), 200);
    let list: Value = resp.json().await.unwrap();
    let ids: Vec<&str> = list.as_array().unwrap().iter()
        .map(|s| s["id"].as_str().unwrap()).collect();
    assert!(ids.contains(&skill_id.as_str()));

    // Get
    let resp = env.client.get(format!("{base}/skills/{skill_id}")).send().await.unwrap();
    assert_eq!(resp.status().as_u16(), 200);
    let got: Value = resp.json().await.unwrap();
    assert_eq!(got["id"], skill_id);

    // Get non-existent → 404
    let resp = env.client.get(format!("{base}/skills/skill_nonexistent")).send().await.unwrap();
    assert_eq!(resp.status().as_u16(), 404);

    // List versions — one entry
    let resp = env.client.get(format!("{base}/skills/{skill_id}/versions")).send().await.unwrap();
    assert_eq!(resp.status().as_u16(), 200);
    let versions: Value = resp.json().await.unwrap();
    assert_eq!(versions.as_array().unwrap().len(), 1);
    assert_eq!(versions[0]["content"], "## PDF Extractor\nUse this skill to extract text.");
    assert_eq!(versions[0]["is_latest"], true);

    // Create new version
    let resp = env.client.post(format!("{base}/skills/{skill_id}/versions"))
        .json(&json!({ "content": "## PDF Extractor v2\nImproved extraction." }))
        .send().await.unwrap();
    assert_eq!(resp.status().as_u16(), 201);
    let new_ver: Value = resp.json().await.unwrap();
    assert_eq!(new_ver["is_latest"], true);

    // Get skill — latest_version updated
    let resp = env.client.get(format!("{base}/skills/{skill_id}")).send().await.unwrap();
    let got: Value = resp.json().await.unwrap();
    let latest = got["latest_version"].as_str().unwrap();
    assert_eq!(latest.len(), 8);

    // Both version creates happen on the same day so they share the same YYYYMMDD key
    // in the versions KV — the second put overwrites the first. Only 1 entry is expected.
    // The content should reflect the latest version.
    let resp = env.client.get(format!("{base}/skills/{skill_id}/versions")).send().await.unwrap();
    let versions: Value = resp.json().await.unwrap();
    assert_eq!(versions.as_array().unwrap().len(), 1);
    assert_eq!(versions[0]["content"], "## PDF Extractor v2\nImproved extraction.");
}

// ── Session round-trip (write RawSession → read via console API) ──────────────

#[tokio::test]
async fn session_read_reflects_token_counts_and_agent_id() {
    let env = start().await;
    let base = &env.base_url;

    // Write a RawSession directly into the SESSIONS KV bucket (simulating trogon-agent).
    // The key format is "{tenant_id}.{session_id}".
    let tenant_id = "tenant_acme";
    let session_id = "sess_abc123";

    let raw_session = json!({
        "id": session_id,
        "tenant_id": tenant_id,
        "name": "Test session",
        "model": "claude-sonnet-4-6",
        "messages": [
            {
                "role": "user",
                "content": [{ "type": "text", "text": "Hello" }]
            },
            {
                "role": "assistant",
                "content": [{ "type": "text", "text": "Hi there!" }],
                "usage": {
                    "input_tokens": 10,
                    "output_tokens": 5,
                    "cache_creation_input_tokens": 3,
                    "cache_read_input_tokens": 2
                }
            },
            {
                "role": "user",
                "content": [{ "type": "text", "text": "Follow-up" }]
            },
            {
                "role": "assistant",
                "content": [{ "type": "text", "text": "Sure!" }],
                "usage": {
                    "input_tokens": 15,
                    "output_tokens": 3
                }
            }
        ],
        "created_at": "1776384000",
        "updated_at": "1776384100",
        "duration_ms": 42000,
        "agent_id": "agent_deadadd"
    });

    let sessions_kv = env.js
        .create_or_update_key_value(async_nats::jetstream::kv::Config {
            bucket: "SESSIONS".to_string(),
            history: 1,
            ..Default::default()
        })
        .await
        .expect("open SESSIONS KV");

    let key = format!("{tenant_id}.{session_id}");
    let bytes = serde_json::to_vec(&raw_session).unwrap();
    sessions_kv.put(&key, Bytes::from(bytes)).await.expect("put session");

    // GET via console API
    let resp = env.client
        .get(format!("{base}/sessions/{tenant_id}/{session_id}"))
        .send().await.unwrap();
    assert_eq!(resp.status().as_u16(), 200);
    let s: Value = resp.json().await.unwrap();

    assert_eq!(s["id"], session_id);
    assert_eq!(s["tenant_id"], tenant_id);
    assert_eq!(s["message_count"], 4);
    // Token sums from both assistant messages
    assert_eq!(s["input_tokens"], 25,  "10 + 15");
    assert_eq!(s["output_tokens"], 8,  "5 + 3");
    assert_eq!(s["cache_write_tokens"], 3, "cache_creation from first message");
    assert_eq!(s["cache_read_tokens"], 2,  "cache_read from first message");
    assert_eq!(s["duration_ms"], 42000);
    assert_eq!(s["agent_id"], "agent_deadadd");
    // Last message is from assistant → status = idle
    assert_eq!(s["status"], "idle");
}

#[tokio::test]
async fn session_list_returns_all_sessions() {
    let env = start().await;
    let base = &env.base_url;

    let sessions_kv = env.js
        .create_or_update_key_value(async_nats::jetstream::kv::Config {
            bucket: "SESSIONS".to_string(),
            history: 1,
            ..Default::default()
        })
        .await
        .expect("open SESSIONS KV");

    for i in 0..3u32 {
        let s = json!({
            "id": format!("sess_{i}"),
            "tenant_id": "tenant_x",
            "name": format!("Session {i}"),
            "messages": [],
            "created_at": "1776384000",
            "updated_at": format!("{}", 1776384000 + i)
        });
        let key = format!("tenant_x.sess_{i}");
        sessions_kv.put(&key, Bytes::from(serde_json::to_vec(&s).unwrap())).await.unwrap();
    }

    let resp = env.client.get(format!("{base}/sessions")).send().await.unwrap();
    assert_eq!(resp.status().as_u16(), 200);
    let list: Value = resp.json().await.unwrap();
    assert_eq!(list.as_array().unwrap().len(), 3);
}

#[tokio::test]
async fn session_get_nonexistent_returns_404() {
    let env = start().await;
    let resp = env.client
        .get(format!("{}/sessions/no_tenant/no_session", env.base_url))
        .send().await.unwrap();
    assert_eq!(resp.status().as_u16(), 404);
}

// ── Agent sessions list ───────────────────────────────────────────────────────

#[tokio::test]
async fn agent_sessions_lists_sessions_by_tenant() {
    let env = start().await;
    let base = &env.base_url;

    // Create an agent
    let agent_body = json!({
        "name": "BotAgent",
        "description": "A bot",
        "model": { "id": "claude-haiku-4-5-20251001" },
        "system_prompt": "Be a bot.",
    });
    let resp = env.client.post(format!("{base}/agents"))
        .json(&agent_body).send().await.unwrap();
    let agent: Value = resp.json().await.unwrap();
    let agent_id = agent["id"].as_str().unwrap();

    // Write sessions for this agent (tenant_id = agent_id in this route)
    let sessions_kv = env.js
        .create_or_update_key_value(async_nats::jetstream::kv::Config {
            bucket: "SESSIONS".to_string(),
            history: 1,
            ..Default::default()
        })
        .await
        .unwrap();

    for i in 0..2u32 {
        let s = json!({
            "id": format!("sess_{i}"),
            "tenant_id": agent_id,
            "name": format!("Session {i}"),
            "messages": [],
            "created_at": "1776384000",
            "updated_at": "1776384000"
        });
        let key = format!("{agent_id}.sess_{i}");
        sessions_kv.put(&key, Bytes::from(serde_json::to_vec(&s).unwrap())).await.unwrap();
    }

    // Also write a session for a different tenant — should not appear
    let other = json!({
        "id": "sess_other",
        "tenant_id": "other_tenant",
        "name": "Other",
        "messages": [],
        "created_at": "1776384000",
        "updated_at": "1776384000"
    });
    sessions_kv.put("other_tenant.sess_other", Bytes::from(serde_json::to_vec(&other).unwrap())).await.unwrap();

    let resp = env.client.get(format!("{base}/agents/{agent_id}/sessions")).send().await.unwrap();
    assert_eq!(resp.status().as_u16(), 200);
    let list: Value = resp.json().await.unwrap();
    assert_eq!(list.as_array().unwrap().len(), 2, "should return only sessions for this agent/tenant");
}

// ── Environment CRUD ──────────────────────────────────────────────────────────

#[tokio::test]
async fn environment_create_list_get_update_delete() {
    let env = start().await;
    let base = &env.base_url;

    // Create
    let body = json!({
        "name": "Production",
        "description": "Prod environment",
        "type": "cloud",
        "networking": "restricted"
    });
    let resp = env.client.post(format!("{base}/environments"))
        .json(&body).send().await.unwrap();
    assert_eq!(resp.status().as_u16(), 201);
    let created: Value = resp.json().await.unwrap();
    let env_id = created["id"].as_str().unwrap().to_string();
    assert!(env_id.starts_with("env_"));
    assert_eq!(created["name"], "Production");
    assert_eq!(created["type"], "cloud");
    assert_eq!(created["networking"], "restricted");
    assert_eq!(created["archived"], false);

    // List — environment appears
    let resp = env.client.get(format!("{base}/environments")).send().await.unwrap();
    assert_eq!(resp.status().as_u16(), 200);
    let list: Value = resp.json().await.unwrap();
    let ids: Vec<&str> = list.as_array().unwrap().iter()
        .map(|e| e["id"].as_str().unwrap()).collect();
    assert!(ids.contains(&env_id.as_str()));

    // Get
    let resp = env.client.get(format!("{base}/environments/{env_id}")).send().await.unwrap();
    assert_eq!(resp.status().as_u16(), 200);
    let got: Value = resp.json().await.unwrap();
    assert_eq!(got["id"], env_id);
    assert_eq!(got["description"], "Prod environment");

    // Get non-existent → 404
    let resp = env.client.get(format!("{base}/environments/env_nope")).send().await.unwrap();
    assert_eq!(resp.status().as_u16(), 404);

    // Update
    let resp = env.client.put(format!("{base}/environments/{env_id}"))
        .json(&json!({ "name": "Production-v2", "networking": "unrestricted" }))
        .send().await.unwrap();
    assert_eq!(resp.status().as_u16(), 200);
    let updated: Value = resp.json().await.unwrap();
    assert_eq!(updated["name"], "Production-v2");
    assert_eq!(updated["networking"], "unrestricted");
    assert_eq!(updated["type"], "cloud", "type unchanged");

    // Delete
    let resp = env.client.delete(format!("{base}/environments/{env_id}")).send().await.unwrap();
    assert_eq!(resp.status().as_u16(), 204);

    // Get after delete → 404
    let resp = env.client.get(format!("{base}/environments/{env_id}")).send().await.unwrap();
    assert_eq!(resp.status().as_u16(), 404);
}

#[tokio::test]
async fn environment_archive_sets_archived_flag() {
    let env = start().await;
    let base = &env.base_url;

    let resp = env.client.post(format!("{base}/environments"))
        .json(&json!({ "name": "ToArchive" }))
        .send().await.unwrap();
    let created: Value = resp.json().await.unwrap();
    let env_id = created["id"].as_str().unwrap();
    assert_eq!(created["archived"], false);

    // Archive
    let resp = env.client.post(format!("{base}/environments/{env_id}/archive"))
        .send().await.unwrap();
    assert_eq!(resp.status().as_u16(), 200);
    let archived: Value = resp.json().await.unwrap();
    assert_eq!(archived["archived"], true);

    // Verify persisted — GET reflects the flag
    let resp = env.client.get(format!("{base}/environments/{env_id}")).send().await.unwrap();
    let got: Value = resp.json().await.unwrap();
    assert_eq!(got["archived"], true);
}

#[tokio::test]
async fn environment_update_nonexistent_returns_404() {
    let env = start().await;
    let resp = env.client
        .put(format!("{}/environments/env_ghost", env.base_url))
        .json(&json!({ "name": "Ghost" }))
        .send().await.unwrap();
    assert_eq!(resp.status().as_u16(), 404);
}

#[tokio::test]
async fn environment_create_applies_defaults() {
    let env = start().await;
    let base = &env.base_url;

    // Only required field: name
    let resp = env.client.post(format!("{base}/environments"))
        .json(&json!({ "name": "Minimal" }))
        .send().await.unwrap();
    assert_eq!(resp.status().as_u16(), 201);
    let created: Value = resp.json().await.unwrap();
    assert_eq!(created["type"], "cloud",          "default type is cloud");
    assert_eq!(created["networking"], "unrestricted", "default networking is unrestricted");
    assert_eq!(created["archived"], false);
    assert_eq!(created["packages"], json!([]));
}

// ── Credentials ───────────────────────────────────────────────────────────────

#[tokio::test]
async fn credential_vault_not_created_until_first_credential() {
    let env = start().await;
    let base = &env.base_url;

    // Create an environment first
    let resp = env.client.post(format!("{base}/environments"))
        .json(&json!({ "name": "VaultTestEnv" }))
        .send().await.unwrap();
    let created_env: Value = resp.json().await.unwrap();
    let env_id = created_env["id"].as_str().unwrap();

    // No vault yet → 404
    let resp = env.client.get(format!("{base}/environments/{env_id}/vault")).send().await.unwrap();
    assert_eq!(resp.status().as_u16(), 404, "vault should not exist before first credential");

    // Create credential — auto-creates vault
    let resp = env.client.post(format!("{base}/environments/{env_id}/credentials"))
        .json(&json!({
            "name": "GitHub Token",
            "type": "bearer_token",
            "mcp_server_url": "https://api.github.com"
        }))
        .send().await.unwrap();
    assert_eq!(resp.status().as_u16(), 201);

    // Vault now exists
    let resp = env.client.get(format!("{base}/environments/{env_id}/vault")).send().await.unwrap();
    assert_eq!(resp.status().as_u16(), 200);
    let vault: Value = resp.json().await.unwrap();
    assert_eq!(vault["env_id"], env_id);
    assert!(vault["id"].as_str().unwrap().starts_with("vlt_"));
}

#[tokio::test]
async fn credential_create_list_delete() {
    let env = start().await;
    let base = &env.base_url;

    let resp = env.client.post(format!("{base}/environments"))
        .json(&json!({ "name": "CredEnv" }))
        .send().await.unwrap();
    let env_obj: Value = resp.json().await.unwrap();
    let env_id = env_obj["id"].as_str().unwrap();

    // List credentials — empty initially
    let resp = env.client.get(format!("{base}/environments/{env_id}/credentials")).send().await.unwrap();
    assert_eq!(resp.status().as_u16(), 200);
    assert_eq!(resp.json::<Value>().await.unwrap().as_array().unwrap().len(), 0);

    // Create two credentials
    let mut cred_ids = vec![];
    for name in ["Linear API Key", "Slack Bot Token"] {
        let resp = env.client.post(format!("{base}/environments/{env_id}/credentials"))
            .json(&json!({
                "name": name,
                "type": "bearer_token",
                "mcp_server_url": "https://example.com"
            }))
            .send().await.unwrap();
        assert_eq!(resp.status().as_u16(), 201);
        let cred: Value = resp.json().await.unwrap();
        assert!(cred["id"].as_str().unwrap().starts_with("crd_"));
        assert_eq!(cred["env_id"], env_id);
        assert_eq!(cred["status"], "active");
        cred_ids.push(cred["id"].as_str().unwrap().to_string());
    }

    // List — two credentials
    let resp = env.client.get(format!("{base}/environments/{env_id}/credentials")).send().await.unwrap();
    assert_eq!(resp.status().as_u16(), 200);
    let list: Value = resp.json().await.unwrap();
    assert_eq!(list.as_array().unwrap().len(), 2);

    // Delete first credential
    let resp = env.client
        .delete(format!("{base}/environments/{env_id}/credentials/{}", cred_ids[0]))
        .send().await.unwrap();
    assert_eq!(resp.status().as_u16(), 204);

    // List — one credential left
    let resp = env.client.get(format!("{base}/environments/{env_id}/credentials")).send().await.unwrap();
    let list: Value = resp.json().await.unwrap();
    assert_eq!(list.as_array().unwrap().len(), 1);
    assert_eq!(list[0]["id"], cred_ids[1]);
}

#[tokio::test]
async fn credential_isolation_between_environments() {
    let env = start().await;
    let base = &env.base_url;

    // Create two environments
    let env_a_id = {
        let resp = env.client.post(format!("{base}/environments"))
            .json(&json!({ "name": "EnvA" })).send().await.unwrap();
        resp.json::<Value>().await.unwrap()["id"].as_str().unwrap().to_string()
    };
    let env_b_id = {
        let resp = env.client.post(format!("{base}/environments"))
            .json(&json!({ "name": "EnvB" })).send().await.unwrap();
        resp.json::<Value>().await.unwrap()["id"].as_str().unwrap().to_string()
    };

    // Add 2 credentials to env_A, 1 to env_B
    for name in ["A-cred-1", "A-cred-2"] {
        env.client.post(format!("{base}/environments/{env_a_id}/credentials"))
            .json(&json!({ "name": name, "type": "bearer_token", "mcp_server_url": "https://a.example.com" }))
            .send().await.unwrap();
    }
    env.client.post(format!("{base}/environments/{env_b_id}/credentials"))
        .json(&json!({ "name": "B-cred-1", "type": "bearer_token", "mcp_server_url": "https://b.example.com" }))
        .send().await.unwrap();

    // env_A sees 2, env_B sees 1 — no cross-contamination
    let list_a: Value = env.client
        .get(format!("{base}/environments/{env_a_id}/credentials"))
        .send().await.unwrap().json().await.unwrap();
    assert_eq!(list_a.as_array().unwrap().len(), 2, "env_A should have 2 credentials");

    let list_b: Value = env.client
        .get(format!("{base}/environments/{env_b_id}/credentials"))
        .send().await.unwrap().json().await.unwrap();
    assert_eq!(list_b.as_array().unwrap().len(), 1, "env_B should have 1 credential");

    // Verify env_A creds all belong to env_A
    for cred in list_a.as_array().unwrap() {
        assert_eq!(cred["env_id"], env_a_id, "credential env_id mismatch");
    }
}

// ── Agent additional behaviors ────────────────────────────────────────────────

#[tokio::test]
async fn agent_partial_update_preserves_unchanged_fields() {
    let env = start().await;
    let base = &env.base_url;

    let body = json!({
        "name": "OriginalName",
        "description": "Original description",
        "model": { "id": "claude-opus-4-7" },
        "system_prompt": "Be helpful.",
        "skill_ids": ["skill_a", "skill_b"],
        "mcp_servers": ["mcp_server_1"]
    });
    let resp = env.client.post(format!("{base}/agents")).json(&body).send().await.unwrap();
    let created: Value = resp.json().await.unwrap();
    let agent_id = created["id"].as_str().unwrap();

    // Update only the name
    let resp = env.client.put(format!("{base}/agents/{agent_id}"))
        .json(&json!({ "name": "RenamedAgent" }))
        .send().await.unwrap();
    assert_eq!(resp.status().as_u16(), 200);
    let updated: Value = resp.json().await.unwrap();

    assert_eq!(updated["name"], "RenamedAgent",                      "name was updated");
    assert_eq!(updated["description"], "Original description",        "description unchanged");
    assert_eq!(updated["model"]["id"], "claude-opus-4-7",            "model unchanged");
    assert_eq!(updated["system_prompt"], "Be helpful.",              "system_prompt unchanged");
    assert_eq!(updated["skill_ids"], json!(["skill_a", "skill_b"]),  "skill_ids unchanged");
    assert_eq!(updated["mcp_servers"], json!(["mcp_server_1"]),      "mcp_servers unchanged");
    assert_eq!(updated["version"], 2,                                "version bumped");
}

#[tokio::test]
async fn agent_update_nonexistent_returns_404() {
    let env = start().await;
    let resp = env.client
        .put(format!("{}/agents/agent_ghost", env.base_url))
        .json(&json!({ "name": "Ghost" }))
        .send().await.unwrap();
    assert_eq!(resp.status().as_u16(), 404);
}

#[tokio::test]
async fn agent_delete_nonexistent_returns_204() {
    let env = start().await;
    // NATS KV delete publishes a tombstone regardless of whether the key exists —
    // it is idempotent by design. The route therefore returns 204 (not 404) for
    // non-existent agents. This is the documented, expected behavior.
    let resp = env.client
        .delete(format!("{}/agents/agent_ghost", env.base_url))
        .send().await.unwrap();
    assert_eq!(resp.status().as_u16(), 204);
}

#[tokio::test]
async fn agent_list_sorted_by_updated_at_desc() {
    let env = start().await;
    let base = &env.base_url;

    // Create three agents
    let mut agent_ids = vec![];
    for name in ["Agent-A", "Agent-B", "Agent-C"] {
        let resp = env.client.post(format!("{base}/agents"))
            .json(&json!({
                "name": name,
                "description": "",
                "model": { "id": "claude-haiku-4-5-20251001" },
                "system_prompt": "."
            }))
            .send().await.unwrap();
        let a: Value = resp.json().await.unwrap();
        agent_ids.push(a["id"].as_str().unwrap().to_string());
    }

    // Sleep 1s so the update lands on a different epoch-second than the creates
    tokio::time::sleep(std::time::Duration::from_secs(1)).await;

    // Update the first agent — it should now appear first in the list
    env.client.put(format!("{base}/agents/{}", agent_ids[0]))
        .json(&json!({ "name": "Agent-A-Updated" }))
        .send().await.unwrap();

    let resp = env.client.get(format!("{base}/agents")).send().await.unwrap();
    let list: Value = resp.json().await.unwrap();
    let arr = list.as_array().unwrap();
    assert_eq!(arr[0]["id"], agent_ids[0], "most recently updated agent must be first");
}

// ── Skill additional behaviors ────────────────────────────────────────────────

#[tokio::test]
async fn skill_create_with_custom_provider() {
    let env = start().await;
    let base = &env.base_url;

    let resp = env.client.post(format!("{base}/skills"))
        .json(&json!({
            "name": "anthropic-summarizer",
            "description": "Summarization skill",
            "provider": "anthropic",
            "content": "## Summarizer\nCondense text."
        }))
        .send().await.unwrap();
    assert_eq!(resp.status().as_u16(), 201);
    let created: Value = resp.json().await.unwrap();
    assert_eq!(created["provider"], "anthropic", "custom provider must be stored as-is");

    // Round-trip: GET returns the same provider
    let skill_id = created["id"].as_str().unwrap();
    let got: Value = env.client.get(format!("{base}/skills/{skill_id}"))
        .send().await.unwrap().json().await.unwrap();
    assert_eq!(got["provider"], "anthropic");
}

#[tokio::test]
async fn skill_versions_for_nonexistent_skill_returns_empty_list() {
    let env = start().await;
    // No 404 — the implementation uses a keys() prefix scan which returns nothing
    // for an unknown skill_id. This is the documented behavior.
    let resp = env.client
        .get(format!("{}/skills/skill_nonexistent/versions", env.base_url))
        .send().await.unwrap();
    assert_eq!(resp.status().as_u16(), 200);
    let versions: Value = resp.json().await.unwrap();
    assert_eq!(versions.as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn skill_create_version_for_nonexistent_returns_404() {
    let env = start().await;
    let resp = env.client
        .post(format!("{}/skills/skill_ghost/versions", env.base_url))
        .json(&json!({ "content": "# Ghost skill" }))
        .send().await.unwrap();
    assert_eq!(resp.status().as_u16(), 404);
}

// ── Session additional behaviors ──────────────────────────────────────────────

#[tokio::test]
async fn session_status_running_when_last_message_is_user() {
    let env = start().await;
    let base = &env.base_url;

    let sessions_kv = env.js
        .create_or_update_key_value(async_nats::jetstream::kv::Config {
            bucket: "SESSIONS".to_string(),
            history: 1,
            ..Default::default()
        })
        .await.unwrap();

    let raw = json!({
        "id": "sess_running",
        "tenant_id": "t1",
        "name": "Running session",
        "messages": [
            { "role": "user", "content": [{ "type": "text", "text": "Hello" }] },
            { "role": "assistant", "content": [{ "type": "text", "text": "Hi" }] },
            { "role": "user", "content": [{ "type": "text", "text": "Follow-up?" }] }
        ],
        "created_at": "1776384000",
        "updated_at": "1776384001"
    });
    sessions_kv.put("t1.sess_running", Bytes::from(serde_json::to_vec(&raw).unwrap())).await.unwrap();

    let resp = env.client.get(format!("{base}/sessions/t1/sess_running")).send().await.unwrap();
    assert_eq!(resp.status().as_u16(), 200);
    let s: Value = resp.json().await.unwrap();
    assert_eq!(s["status"], "running", "last message from user → status must be running");
    assert_eq!(s["message_count"], 3);
}

#[tokio::test]
async fn session_with_no_messages_has_zero_counts_and_idle_status() {
    let env = start().await;
    let base = &env.base_url;

    let sessions_kv = env.js
        .create_or_update_key_value(async_nats::jetstream::kv::Config {
            bucket: "SESSIONS".to_string(),
            history: 1,
            ..Default::default()
        })
        .await.unwrap();

    let raw = json!({
        "id": "sess_empty",
        "tenant_id": "t2",
        "name": "Empty session",
        "messages": [],
        "created_at": "1776384000",
        "updated_at": "1776384000"
    });
    sessions_kv.put("t2.sess_empty", Bytes::from(serde_json::to_vec(&raw).unwrap())).await.unwrap();

    let resp = env.client.get(format!("{base}/sessions/t2/sess_empty")).send().await.unwrap();
    assert_eq!(resp.status().as_u16(), 200);
    let s: Value = resp.json().await.unwrap();
    assert_eq!(s["status"], "idle");
    assert_eq!(s["message_count"], 0);
    assert_eq!(s["input_tokens"], 0);
    assert_eq!(s["output_tokens"], 0);
    assert_eq!(s["cache_read_tokens"], 0);
    assert_eq!(s["cache_write_tokens"], 0);
    assert_eq!(s["duration_ms"], 0);
    assert_eq!(s["agent_id"], Value::Null);
}

#[tokio::test]
async fn session_list_sorted_by_updated_at_desc() {
    let env = start().await;
    let base = &env.base_url;

    let sessions_kv = env.js
        .create_or_update_key_value(async_nats::jetstream::kv::Config {
            bucket: "SESSIONS".to_string(),
            history: 1,
            ..Default::default()
        })
        .await.unwrap();

    // Write sessions with explicit updated_at epoch values — deliberately out of order
    let data = [
        ("sess_old",    "1776384001"),
        ("sess_newest", "1776384003"),
        ("sess_mid",    "1776384002"),
    ];
    for (id, updated_at) in data {
        let s = json!({
            "id": id,
            "tenant_id": "sort_tenant",
            "name": id,
            "messages": [],
            "created_at": "1776384000",
            "updated_at": updated_at
        });
        let key = format!("sort_tenant.{id}");
        sessions_kv.put(&key, Bytes::from(serde_json::to_vec(&s).unwrap())).await.unwrap();
    }

    let resp = env.client.get(format!("{base}/sessions")).send().await.unwrap();
    assert_eq!(resp.status().as_u16(), 200);
    let list: Value = resp.json().await.unwrap();
    let arr = list.as_array().unwrap();
    assert_eq!(arr.len(), 3);
    // Sorted descending by updated_at — newest first
    assert_eq!(arr[0]["id"], "sess_newest", "highest updated_at must be first");
    assert_eq!(arr[1]["id"], "sess_mid");
    assert_eq!(arr[2]["id"], "sess_old");
}

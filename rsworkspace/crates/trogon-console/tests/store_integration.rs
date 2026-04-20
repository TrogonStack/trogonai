use async_nats::jetstream;
use bytes::Bytes;
use testcontainers_modules::nats::Nats;
use testcontainers_modules::testcontainers::{ImageExt, runners::AsyncRunner};

use trogon_console::models::agent::{AgentDefinition, AgentModel, AgentStatus};
use trogon_console::models::credential::{Credential, CredentialStatus, CredentialType};
use trogon_console::models::environment::{Environment, EnvironmentType, NetworkingType};
use trogon_console::models::skill::{Skill, SkillVersion};
use trogon_console::store::agents::{AgentStore, AGENT_VERSIONS_BUCKET, AGENTS_BUCKET};
use trogon_console::store::credentials::{CredentialStore, CREDS_BUCKET, VAULTS_BUCKET};
use trogon_console::store::environments::{EnvironmentStore, ENVS_BUCKET};
use trogon_console::store::sessions::{SessionReader, SESSIONS_BUCKET};
use trogon_console::store::skills::{SkillStore, SKILL_VERSIONS_BUCKET, SKILLS_BUCKET};

async fn nats_js() -> (jetstream::Context, Box<dyn std::any::Any>) {
    let container = Nats::default()
        .with_cmd(["--jetstream"])
        .start()
        .await
        .expect("NATS container start");
    let port = container.get_host_port_ipv4(4222).await.expect("port");
    let nats = async_nats::connect(format!("nats://127.0.0.1:{port}"))
        .await
        .expect("NATS connect");
    (jetstream::new(nats), Box::new(container))
}

fn make_agent(id: &str, updated_at: &str) -> AgentDefinition {
    AgentDefinition {
        id: id.to_string(),
        name: format!("Agent {id}"),
        description: "test".to_string(),
        status: AgentStatus::Active,
        version: 1,
        model: AgentModel { id: "claude-3".to_string(), speed: "standard".to_string() },
        system_prompt: "You are helpful".to_string(),
        skill_ids: vec![],
        tools: vec![],
        mcp_servers: vec![],
        metadata: serde_json::Value::Null,
        created_at: updated_at.to_string(),
        updated_at: updated_at.to_string(),
    }
}

fn make_skill(id: &str) -> Skill {
    Skill {
        id: id.to_string(),
        name: format!("Skill {id}"),
        description: "test skill".to_string(),
        provider: "custom".to_string(),
        latest_version: "20260101".to_string(),
        created_at: "2026-01-01".to_string(),
        updated_at: "2026-01-01".to_string(),
    }
}

fn make_credential(id: &str, env_id: &str, vault_id: &str) -> Credential {
    Credential {
        id: id.to_string(),
        vault_id: vault_id.to_string(),
        env_id: env_id.to_string(),
        name: format!("Cred {id}"),
        credential_type: CredentialType::BearerToken,
        mcp_server_url: "https://example.com".to_string(),
        status: CredentialStatus::Active,
        rotation_policy_days: None,
        created_at: "2026-01-01".to_string(),
        updated_at: "2026-01-01".to_string(),
    }
}

fn make_env(id: &str, name: &str) -> Environment {
    Environment {
        id: id.to_string(),
        name: name.to_string(),
        description: String::new(),
        env_type: EnvironmentType::Cloud,
        networking: NetworkingType::Unrestricted,
        packages: vec![],
        metadata: Default::default(),
        archived: false,
        created_at: "2026-01-01".to_string(),
        updated_at: "2026-01-01".to_string(),
    }
}

// ── AgentStore ────────────────────────────────────────────────────────────────

#[tokio::test]
async fn agent_store_list_empty() {
    let (js, _c) = nats_js().await;
    let store = AgentStore::open(&js).await.unwrap();
    assert!(store.list().await.unwrap().is_empty());
}

#[tokio::test]
async fn agent_store_delete_removes_from_list() {
    let (js, _c) = nats_js().await;
    let store = AgentStore::open(&js).await.unwrap();
    store.put(&make_agent("ag1", "2026-01-01")).await.unwrap();
    store.delete("ag1").await.unwrap();
    assert!(store.list().await.unwrap().is_empty());
}

#[tokio::test]
async fn agent_store_list_versions_prefix_filter() {
    let (js, _c) = nats_js().await;
    let store = AgentStore::open(&js).await.unwrap();

    let mut a = make_agent("agent_a", "2026-01-01");
    a.version = 1;
    store.put(&a).await.unwrap();
    a.version = 2;
    store.put(&a).await.unwrap();
    store.put(&make_agent("agent_b", "2026-01-02")).await.unwrap();

    let versions_a = store.list_versions("agent_a").await.unwrap();
    assert_eq!(versions_a.len(), 2);
    assert!(versions_a.iter().all(|v| v.version <= 2));

    let versions_b = store.list_versions("agent_b").await.unwrap();
    assert_eq!(versions_b.len(), 1);
}

#[tokio::test]
async fn agent_store_get_invalid_json_returns_error() {
    let (js, _c) = nats_js().await;
    let store = AgentStore::open(&js).await.unwrap();
    let kv = js.get_key_value(AGENTS_BUCKET).await.unwrap();
    kv.put("bad_agent", Bytes::from_static(b"not valid json")).await.unwrap();
    assert!(store.get("bad_agent").await.is_err());
}

// ── SkillStore ────────────────────────────────────────────────────────────────

#[tokio::test]
async fn skill_store_delete() {
    let (js, _c) = nats_js().await;
    let store = SkillStore::open(&js).await.unwrap();
    store.put(&make_skill("sk1")).await.unwrap();
    assert!(store.get("sk1").await.unwrap().is_some());
    store.delete("sk1").await.unwrap();
    assert!(store.get("sk1").await.unwrap().is_none());
}

#[tokio::test]
async fn skill_store_list_versions_prefix_filter() {
    let (js, _c) = nats_js().await;
    let store = SkillStore::open(&js).await.unwrap();

    let v = |skill_id: &str, version: &str| SkillVersion {
        skill_id: skill_id.to_string(),
        version: version.to_string(),
        content: "content".to_string(),
        is_latest: false,
        created_at: "2026-01-01".to_string(),
    };

    store.put_version(&v("pdf", "20260101")).await.unwrap();
    store.put_version(&v("pdf", "20260102")).await.unwrap();
    store.put_version(&v("csv", "20260101")).await.unwrap();

    let pdf = store.list_versions("pdf").await.unwrap();
    assert_eq!(pdf.len(), 2);
    // sorted descending by version string
    assert_eq!(pdf[0].version, "20260102");

    let csv = store.list_versions("csv").await.unwrap();
    assert_eq!(csv.len(), 1);
}

#[tokio::test]
async fn skill_store_get_invalid_json_returns_error() {
    let (js, _c) = nats_js().await;
    let store = SkillStore::open(&js).await.unwrap();
    let kv = js.get_key_value(SKILLS_BUCKET).await.unwrap();
    kv.put("bad_skill", Bytes::from_static(b"{broken")).await.unwrap();
    assert!(store.get("bad_skill").await.is_err());
}

// ── CredentialStore ───────────────────────────────────────────────────────────

#[tokio::test]
async fn credential_store_get_nonexistent_and_found() {
    let (js, _c) = nats_js().await;
    let store = CredentialStore::open(&js).await.unwrap();

    assert!(store.get("env1", "nope").await.unwrap().is_none());

    store.put(&make_credential("c1", "env1", "vlt1")).await.unwrap();
    let found = store.get("env1", "c1").await.unwrap();
    assert!(found.is_some());
    assert_eq!(found.unwrap().id, "c1");
}

#[tokio::test]
async fn credential_store_get_vault_returns_none_if_absent() {
    let (js, _c) = nats_js().await;
    let store = CredentialStore::open(&js).await.unwrap();
    assert!(store.get_vault("env_missing").await.unwrap().is_none());
}

#[tokio::test]
async fn credential_store_get_vault_returns_existing_after_create() {
    let (js, _c) = nats_js().await;
    let store = CredentialStore::open(&js).await.unwrap();
    let created = store.get_or_create_vault("env_x").await.unwrap();
    let found = store.get_vault("env_x").await.unwrap().expect("vault should exist");
    assert_eq!(found.id, created.id);
}

#[tokio::test]
async fn credential_store_list_prefix_isolation() {
    let (js, _c) = nats_js().await;
    let store = CredentialStore::open(&js).await.unwrap();
    store.put(&make_credential("c1", "env_a", "vlt_a")).await.unwrap();
    store.put(&make_credential("c2", "env_a", "vlt_a")).await.unwrap();
    store.put(&make_credential("c3", "env_b", "vlt_b")).await.unwrap();

    assert_eq!(store.list("env_a").await.unwrap().len(), 2);
    assert_eq!(store.list("env_b").await.unwrap().len(), 1);
}

// ── EnvironmentStore ──────────────────────────────────────────────────────────

#[tokio::test]
async fn env_store_list_sorted_by_name() {
    let (js, _c) = nats_js().await;
    let store = EnvironmentStore::open(&js).await.unwrap();
    store.put(&make_env("e3", "Zeta")).await.unwrap();
    store.put(&make_env("e1", "Alpha")).await.unwrap();
    store.put(&make_env("e2", "Beta")).await.unwrap();
    let envs = store.list().await.unwrap();
    let names: Vec<&str> = envs.iter().map(|e| e.name.as_str()).collect();
    assert_eq!(names, ["Alpha", "Beta", "Zeta"]);
}

#[tokio::test]
async fn env_store_get_invalid_json_returns_error() {
    let (js, _c) = nats_js().await;
    let store = EnvironmentStore::open(&js).await.unwrap();
    let kv = js.get_key_value(ENVS_BUCKET).await.unwrap();
    kv.put("bad_env", Bytes::from_static(b"oops")).await.unwrap();
    assert!(store.get("bad_env").await.is_err());
}

// ── SessionReader ─────────────────────────────────────────────────────────────

#[tokio::test]
async fn session_reader_get_nonexistent() {
    let (js, _c) = nats_js().await;
    let reader = SessionReader::open(&js).await.unwrap();
    assert!(reader.get("no_tenant", "no_session").await.unwrap().is_none());
}

#[tokio::test]
async fn session_reader_list_ignores_invalid_json() {
    let (js, _c) = nats_js().await;
    let reader = SessionReader::open(&js).await.unwrap();
    let kv = js.get_key_value(SESSIONS_BUCKET).await.unwrap();
    kv.put("tenant.bad_sess", Bytes::from_static(b"garbage")).await.unwrap();
    // Invalid entries are silently skipped
    assert!(reader.list().await.unwrap().is_empty());
}

#[tokio::test]
async fn session_reader_list_by_tenant_filters_correctly() {
    let (js, _c) = nats_js().await;
    let reader = SessionReader::open(&js).await.unwrap();
    let kv = js.get_key_value(SESSIONS_BUCKET).await.unwrap();

    let make_raw = |id: &str, tenant: &str| {
        serde_json::json!({
            "id": id, "tenant_id": tenant, "name": "s",
            "messages": [], "created_at": "t", "updated_at": "t"
        })
    };
    let put = |key: &str, val: serde_json::Value| {
        let bytes = Bytes::from(serde_json::to_vec(&val).unwrap());
        (key.to_string(), bytes)
    };

    let (k1, b1) = put("t_a.s1", make_raw("s1", "t_a"));
    let (k2, b2) = put("t_a.s2", make_raw("s2", "t_a"));
    let (k3, b3) = put("t_b.s3", make_raw("s3", "t_b"));
    kv.put(&k1, b1).await.unwrap();
    kv.put(&k2, b2).await.unwrap();
    kv.put(&k3, b3).await.unwrap();

    let tenant_a = reader.list_by_tenant("t_a").await.unwrap();
    assert_eq!(tenant_a.len(), 2);
    let tenant_b = reader.list_by_tenant("t_b").await.unwrap();
    assert_eq!(tenant_b.len(), 1);
}

// ── silent-skip on bad JSON during list ──────────────────────────────────────

#[tokio::test]
async fn agent_store_list_versions_skips_invalid_json() {
    let (js, _c) = nats_js().await;
    let store = AgentStore::open(&js).await.unwrap();
    let mut a = make_agent("ag_x", "t");
    a.version = 1;
    store.put(&a).await.unwrap();
    // overwrite the version entry with bad JSON
    let kv = js.get_key_value(AGENT_VERSIONS_BUCKET).await.unwrap();
    kv.put("ag_x.v1", Bytes::from_static(b"notjson")).await.unwrap();
    // list_versions silently skips unparseable entries
    let versions = store.list_versions("ag_x").await.unwrap();
    assert!(versions.is_empty());
}

#[tokio::test]
async fn skill_store_list_versions_skips_invalid_json() {
    let (js, _c) = nats_js().await;
    let store = SkillStore::open(&js).await.unwrap();
    // put bad JSON at a key that matches the prefix filter
    let kv = js.get_key_value(SKILL_VERSIONS_BUCKET).await.unwrap();
    kv.put("pdf.20260101", Bytes::from_static(b"notjson")).await.unwrap();
    let versions = store.list_versions("pdf").await.unwrap();
    assert!(versions.is_empty());
}

#[tokio::test]
async fn credential_store_list_skips_invalid_json() {
    let (js, _c) = nats_js().await;
    let store = CredentialStore::open(&js).await.unwrap();
    // put bad JSON at a key matching the prefix
    let kv = js.get_key_value(CREDS_BUCKET).await.unwrap();
    kv.put("env1.crd_bad", Bytes::from_static(b"notjson")).await.unwrap();
    let creds = store.list("env1").await.unwrap();
    assert!(creds.is_empty());
}

// ── NATS error simulation (stream deletion) ───────────────────────────────────

#[tokio::test]
async fn agent_store_nats_error_agents_bucket() {
    let (js, _c) = nats_js().await;
    let store = AgentStore::open(&js).await.unwrap();
    js.delete_stream(format!("KV_{AGENTS_BUCKET}")).await.unwrap();

    assert!(store.get("x").await.is_err());
    assert!(store.delete("x").await.is_err());
    assert!(store.put(&make_agent("x", "t")).await.is_err());
    assert!(store.list().await.is_err());
}

#[tokio::test]
async fn agent_store_nats_error_versions_bucket() {
    let (js, _c) = nats_js().await;
    let store = AgentStore::open(&js).await.unwrap();
    js.delete_stream(format!("KV_{AGENT_VERSIONS_BUCKET}")).await.unwrap();

    assert!(store.list_versions("agent_a").await.is_err());
    // put writes to agents bucket first, then versions bucket
    assert!(store.put(&make_agent("a", "t")).await.is_err());
}

#[tokio::test]
async fn skill_store_nats_error_skills_bucket() {
    let (js, _c) = nats_js().await;
    let store = SkillStore::open(&js).await.unwrap();
    js.delete_stream(format!("KV_{SKILLS_BUCKET}")).await.unwrap();

    assert!(store.get("x").await.is_err());
    assert!(store.put(&make_skill("x")).await.is_err());
    assert!(store.list().await.is_err());
}

#[tokio::test]
async fn skill_store_nats_error_versions_bucket() {
    let (js, _c) = nats_js().await;
    let store = SkillStore::open(&js).await.unwrap();
    js.delete_stream(format!("KV_{SKILL_VERSIONS_BUCKET}")).await.unwrap();

    let sv = SkillVersion {
        skill_id: "sk".to_string(),
        version: "20260101".to_string(),
        content: "c".to_string(),
        is_latest: true,
        created_at: "t".to_string(),
    };
    assert!(store.put_version(&sv).await.is_err());
    assert!(store.list_versions("sk").await.is_err());
}

#[tokio::test]
async fn credential_store_nats_error_creds_bucket() {
    let (js, _c) = nats_js().await;
    let store = CredentialStore::open(&js).await.unwrap();
    js.delete_stream(format!("KV_{CREDS_BUCKET}")).await.unwrap();

    assert!(store.list("env1").await.is_err());
    assert!(store.put(&make_credential("c1", "env1", "vlt1")).await.is_err());
    assert!(store.delete("env1", "c1").await.is_err());
}

#[tokio::test]
async fn credential_store_nats_error_vaults_bucket() {
    let (js, _c) = nats_js().await;
    let store = CredentialStore::open(&js).await.unwrap();
    js.delete_stream(format!("KV_{VAULTS_BUCKET}")).await.unwrap();

    assert!(store.get_vault("env1").await.is_err());
    assert!(store.get_or_create_vault("env1").await.is_err());
}

#[tokio::test]
async fn env_store_nats_error() {
    let (js, _c) = nats_js().await;
    let store = EnvironmentStore::open(&js).await.unwrap();
    js.delete_stream(format!("KV_{ENVS_BUCKET}")).await.unwrap();

    assert!(store.get("x").await.is_err());
    assert!(store.put(&make_env("x", "X")).await.is_err());
    assert!(store.list().await.is_err());
    assert!(store.delete("x").await.is_err());
}

#[tokio::test]
async fn session_store_nats_error() {
    let (js, _c) = nats_js().await;
    let reader = SessionReader::open(&js).await.unwrap();
    js.delete_stream(format!("KV_{SESSIONS_BUCKET}")).await.unwrap();

    assert!(reader.get("t", "s").await.is_err());
    assert!(reader.list().await.is_err());
    assert!(reader.list_by_tenant("t").await.is_err());
}

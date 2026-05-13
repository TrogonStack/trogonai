//! Integration tests for `AgentLoader`, `SkillLoader`, and `NatsSessionStore`.
//!
//! Each test spins up a fresh NATS container (JetStream enabled) so there is
//! no state leakage between tests.

use async_nats::jetstream;
use testcontainers_modules::{
    nats::Nats,
    testcontainers::{ImageExt, runners::AsyncRunner},
};
use trogon_xai_runner::{
    AgentLoader, AgentLoading, SkillLoader, SkillLoading,
};

// ── Helpers ───────────────────────────────────────────────────────────────────

async fn make_js() -> (jetstream::Context, impl Drop) {
    let container = Nats::default()
        .with_cmd(["--jetstream"])
        .start()
        .await
        .expect("start NATS container");
    let port = container.get_host_port_ipv4(4222).await.expect("port");
    let nats = async_nats::connect(format!("nats://127.0.0.1:{port}"))
        .await
        .expect("connect");
    let js = jetstream::new(nats);
    (js, container)
}

/// Write raw JSON bytes into the named KV bucket under `key`.
async fn kv_put(js: &jetstream::Context, bucket: &str, key: &str, json: &str) {
    let kv = js
        .create_or_update_key_value(async_nats::jetstream::kv::Config {
            bucket: bucket.to_string(),
            history: 1,
            ..Default::default()
        })
        .await
        .expect("open KV bucket");
    kv.put(key, bytes::Bytes::from(json.to_string()))
        .await
        .expect("KV put");
}

// ── AgentLoader ───────────────────────────────────────────────────────────────

#[tokio::test]
async fn agent_loader_returns_empty_for_missing_agent() {
    let (js, _c) = make_js().await;
    let loader = AgentLoader::open(&js).await.expect("AgentLoader::open");
    let cfg = loader.load_config("nonexistent").await;
    assert!(cfg.skill_ids.is_empty());
    assert!(cfg.system_prompt.is_none());
    assert!(cfg.model_id.is_none());
}

#[tokio::test]
async fn agent_loader_parses_full_config() {
    let (js, _c) = make_js().await;
    kv_put(
        &js,
        "CONSOLE_AGENTS",
        "agent-1",
        r#"{"skill_ids":["sk1","sk2"],"system_prompt":"Be concise.","model":{"id":"grok-4"}}"#,
    )
    .await;
    let loader = AgentLoader::open(&js).await.expect("open");
    let cfg = loader.load_config("agent-1").await;
    assert_eq!(cfg.skill_ids, vec!["sk1", "sk2"]);
    assert_eq!(cfg.system_prompt.as_deref(), Some("Be concise."));
    assert_eq!(cfg.model_id.as_deref(), Some("grok-4"));
}

#[tokio::test]
async fn agent_loader_returns_empty_for_invalid_json() {
    let (js, _c) = make_js().await;
    kv_put(&js, "CONSOLE_AGENTS", "bad-agent", "not-json").await;
    let loader = AgentLoader::open(&js).await.expect("open");
    let cfg = loader.load_config("bad-agent").await;
    assert!(cfg.skill_ids.is_empty());
}

#[tokio::test]
async fn agent_loader_empty_prompt_and_model_become_none() {
    let (js, _c) = make_js().await;
    kv_put(
        &js,
        "CONSOLE_AGENTS",
        "agent-2",
        r#"{"skill_ids":[],"system_prompt":"","model":{"id":""}}"#,
    )
    .await;
    let loader = AgentLoader::open(&js).await.expect("open");
    let cfg = loader.load_config("agent-2").await;
    assert!(cfg.system_prompt.is_none());
    assert!(cfg.model_id.is_none());
}

// ── SkillLoader ───────────────────────────────────────────────────────────────

#[tokio::test]
async fn skill_loader_returns_none_for_empty_skill_ids() {
    let (js, _c) = make_js().await;
    let loader = SkillLoader::open(&js).await.expect("SkillLoader::open");
    let result = loader.load(&[]).await;
    assert!(result.is_none());
}

#[tokio::test]
async fn skill_loader_returns_none_for_missing_skill() {
    let (js, _c) = make_js().await;
    let loader = SkillLoader::open(&js).await.expect("open");
    let result = loader.load(&["nonexistent".to_string()]).await;
    assert!(result.is_none());
}

#[tokio::test]
async fn skill_loader_loads_single_skill() {
    let (js, _c) = make_js().await;

    kv_put(
        &js,
        "CONSOLE_SKILLS",
        "sk1",
        r#"{"name":"Coding","latest_version":"v1"}"#,
    )
    .await;
    kv_put(
        &js,
        "CONSOLE_SKILL_VERSIONS",
        "sk1.v1",
        r#"{"content":"Always write tests."}"#,
    )
    .await;

    let loader = SkillLoader::open(&js).await.expect("open");
    let text = loader.load(&["sk1".to_string()]).await.unwrap();
    assert!(text.contains("## Skill: Coding"));
    assert!(text.contains("Always write tests."));
    assert!(text.starts_with("# Available Skills"));
}

#[tokio::test]
async fn skill_loader_loads_multiple_skills() {
    let (js, _c) = make_js().await;

    kv_put(
        &js,
        "CONSOLE_SKILLS",
        "sk1",
        r#"{"name":"Alpha","latest_version":"v1"}"#,
    )
    .await;
    kv_put(
        &js,
        "CONSOLE_SKILL_VERSIONS",
        "sk1.v1",
        r#"{"content":"Do alpha things."}"#,
    )
    .await;
    kv_put(
        &js,
        "CONSOLE_SKILLS",
        "sk2",
        r#"{"name":"Beta","latest_version":"v2"}"#,
    )
    .await;
    kv_put(
        &js,
        "CONSOLE_SKILL_VERSIONS",
        "sk2.v2",
        r#"{"content":"Do beta things."}"#,
    )
    .await;

    let loader = SkillLoader::open(&js).await.expect("open");
    let text = loader
        .load(&["sk1".to_string(), "sk2".to_string()])
        .await
        .unwrap();
    assert!(text.contains("## Skill: Alpha"));
    assert!(text.contains("## Skill: Beta"));
    assert!(text.contains("\n\n---\n\n"));
}

#[tokio::test]
async fn skill_loader_skips_skill_with_missing_version_entry() {
    let (js, _c) = make_js().await;

    // Skill meta exists but version content does not.
    kv_put(
        &js,
        "CONSOLE_SKILLS",
        "sk-missing",
        r#"{"name":"Broken","latest_version":"v99"}"#,
    )
    .await;

    let loader = SkillLoader::open(&js).await.expect("open");
    let result = loader.load(&["sk-missing".to_string()]).await;
    assert!(result.is_none());
}

#[tokio::test]
async fn skill_loader_skips_skill_with_empty_content() {
    let (js, _c) = make_js().await;

    kv_put(
        &js,
        "CONSOLE_SKILLS",
        "sk-empty",
        r#"{"name":"Ghost","latest_version":"v1"}"#,
    )
    .await;
    kv_put(
        &js,
        "CONSOLE_SKILL_VERSIONS",
        "sk-empty.v1",
        r#"{"content":""}"#,
    )
    .await;

    let loader = SkillLoader::open(&js).await.expect("open");
    let result = loader.load(&["sk-empty".to_string()]).await;
    assert!(
        result.is_none(),
        "skill with empty content must be filtered out; got: {result:?}"
    );
}

#[tokio::test]
async fn skill_loader_skips_version_with_malformed_json() {
    let (js, _c) = make_js().await;

    kv_put(
        &js,
        "CONSOLE_SKILLS",
        "sk-badver",
        r#"{"name":"BadVer","latest_version":"v1"}"#,
    )
    .await;
    // Version entry exists but is not valid JSON.
    kv_put(&js, "CONSOLE_SKILL_VERSIONS", "sk-badver.v1", "not-json").await;

    let loader = SkillLoader::open(&js).await.expect("open");
    let result = loader.load(&["sk-badver".to_string()]).await;
    assert!(
        result.is_none(),
        "skill with malformed version JSON must be skipped; got: {result:?}"
    );
}

#[tokio::test]
async fn skill_loader_skips_version_with_missing_content_field() {
    let (js, _c) = make_js().await;

    kv_put(
        &js,
        "CONSOLE_SKILLS",
        "sk-nocontent",
        r#"{"name":"NoContent","latest_version":"v1"}"#,
    )
    .await;
    // Version entry is valid JSON but has no "content" key.
    kv_put(
        &js,
        "CONSOLE_SKILL_VERSIONS",
        "sk-nocontent.v1",
        r#"{"other":"data"}"#,
    )
    .await;

    let loader = SkillLoader::open(&js).await.expect("open");
    let result = loader.load(&["sk-nocontent".to_string()]).await;
    assert!(
        result.is_none(),
        "skill with missing content field must be skipped; got: {result:?}"
    );
}

#[tokio::test]
async fn skill_loader_uses_skill_id_as_name_when_name_missing() {
    let (js, _c) = make_js().await;

    // No "name" field — loader should fall back to skill_id.
    kv_put(
        &js,
        "CONSOLE_SKILLS",
        "sk-noname",
        r#"{"latest_version":"v1"}"#,
    )
    .await;
    kv_put(
        &js,
        "CONSOLE_SKILL_VERSIONS",
        "sk-noname.v1",
        r#"{"content":"Some content."}"#,
    )
    .await;

    let loader = SkillLoader::open(&js).await.expect("open");
    let text = loader.load(&["sk-noname".to_string()]).await.unwrap();
    assert!(text.contains("## Skill: sk-noname"));
}

// ── NatsSessionStore ──────────────────────────────────────────────────────────

use trogon_xai_runner::session_store::{
    NatsSessionStore as Store, SessionSnapshot, SessionStoring, SnapshotMessage, TextBlock,
};

fn sample_snapshot(id: &str, tenant_id: &str) -> SessionSnapshot {
    SessionSnapshot {
        id: id.to_string(),
        tenant_id: tenant_id.to_string(),
        name: "Test session".to_string(),
        model: Some("grok-3".to_string()),
        tools: vec![],
        memory_path: None,
        agent_id: None,
        messages: vec![
            SnapshotMessage {
                role: "user".to_string(),
                content: vec![TextBlock::new("Hello")],
                usage: None,
            },
            SnapshotMessage {
                role: "assistant".to_string(),
                content: vec![TextBlock::new("Hi!")],
                usage: None,
            },
        ],
        created_at: "2026-01-01T00:00:00.000Z".to_string(),
        updated_at: "2026-01-01T00:00:00.000Z".to_string(),
        parent_session_id: None,
        branched_at_index: None,
    }
}

#[tokio::test]
async fn session_store_save_and_read_back() {
    let (js, _c) = make_js().await;
    let store = Store::open(&js, 0).await.expect("Store::open");
    let snap = sample_snapshot("sess-1", "acme");

    store.save(&snap).await;

    // Read back raw JSON from KV and verify key fields.
    let kv = js.get_key_value("SESSIONS").await.expect("get KV");
    let bytes = kv.get("acme.sess-1").await.expect("get").expect("exists");
    let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(v["id"], "sess-1");
    assert_eq!(v["tenant_id"], "acme");
    assert_eq!(v["model"], "grok-3");
    assert_eq!(v["messages"][0]["role"], "user");
    assert_eq!(v["messages"][0]["content"][0]["type"], "text");
    assert_eq!(v["messages"][0]["content"][0]["text"], "Hello");
}

#[tokio::test]
async fn session_store_save_overwrites_existing() {
    let (js, _c) = make_js().await;
    let store = Store::open(&js, 0).await.expect("open");

    let snap = sample_snapshot("sess-1", "acme");
    store.save(&snap).await;

    let mut snap2 = snap.clone();
    snap2.name = "Renamed".to_string();
    store.save(&snap2).await;

    let kv = js.get_key_value("SESSIONS").await.expect("get KV");
    let bytes = kv.get("acme.sess-1").await.unwrap().unwrap();
    let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(v["name"], "Renamed");
}

#[tokio::test]
async fn session_store_remove_deletes_key() {
    let (js, _c) = make_js().await;
    let store = Store::open(&js, 0).await.expect("open");
    let snap = sample_snapshot("sess-del", "acme");

    store.save(&snap).await;
    store.remove("acme", "sess-del").await;

    let kv = js.get_key_value("SESSIONS").await.expect("get KV");
    let result = kv.get("acme.sess-del").await.unwrap();
    assert!(result.is_none());
}

#[tokio::test]
async fn session_store_remove_nonexistent_does_not_panic() {
    let (js, _c) = make_js().await;
    let store = Store::open(&js, 0).await.expect("open");
    // Should not panic or return an error.
    store.remove("acme", "never-existed").await;
}

#[tokio::test]
async fn agent_loader_skips_non_string_skill_ids() {
    let (js, _c) = make_js().await;
    kv_put(
        &js,
        "CONSOLE_AGENTS",
        "agent-mixed",
        r#"{"skill_ids":["ok", 42, null, "also-ok"]}"#,
    )
    .await;
    let loader = AgentLoader::open(&js).await.expect("open");
    let cfg = loader.load_config("agent-mixed").await;
    assert_eq!(
        cfg.skill_ids,
        vec!["ok", "also-ok"],
        "non-string skill_ids must be filtered out; got: {:?}",
        cfg.skill_ids
    );
}

#[tokio::test]
async fn session_store_key_format_is_tenant_dot_id() {
    let (js, _c) = make_js().await;
    let store = Store::open(&js, 0).await.expect("open");
    let snap = sample_snapshot("my-session", "my-tenant");

    store.save(&snap).await;

    let kv = js.get_key_value("SESSIONS").await.expect("get KV");
    let result = kv.get("my-tenant.my-session").await.unwrap();
    assert!(result.is_some(), "key 'my-tenant.my-session' must exist");
}

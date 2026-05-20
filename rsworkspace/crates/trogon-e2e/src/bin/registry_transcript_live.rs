//! Live end-to-end test for trogon-registry and trogon-transcript.
//!
//! Requirements:
//!   • NATS running on nats://localhost:4222  (JetStream required)
//!   • No external credentials
//!
//! Run:
//!   cargo run -p trogon-e2e --bin registry_transcript_live

use async_nats::jetstream;
use trogon_registry::{AgentCapability, Registry, provision};
use trogon_transcript::{NatsTranscriptPublisher, Session, TranscriptStore, entry::Role};
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

// ── Registry tests ─────────────────────────────────────────────────────────────

async fn test_registry_register_and_get(js: &jetstream::Context) -> bool {
    const LABEL: &str = "Registry — register + get by agent_type";
    let id = uid();
    let agent_type = format!("TestActor-{id}");

    let result: Result<(), String> = async {
        let store = provision(js).await.map_err(|e| e.to_string())?;
        let registry = Registry::new(store);

        let cap = AgentCapability::new(&agent_type, ["code_review", "security_analysis"], "actors.test.>");
        registry.register(&cap).await.map_err(|e| e.to_string())?;

        let found = registry.get(&agent_type).await.map_err(|e| e.to_string())?;
        let found = found.ok_or_else(|| format!("agent {agent_type} not found after registration"))?;
        if found.agent_type != agent_type { return Err(format!("agent_type mismatch: {} vs {agent_type}", found.agent_type)); }
        if !found.has_capability("code_review") { return Err("expected code_review capability".into()); }
        Ok(())
    }.await;

    match result { Ok(()) => { ok(LABEL); true } Err(e) => { ko(LABEL, &e); false } }
}

async fn test_registry_list_all(js: &jetstream::Context) -> bool {
    const LABEL: &str = "Registry — list_all includes registered agents";
    let id = uid();
    let agent_type = format!("ListActor-{id}");

    let result: Result<(), String> = async {
        let store = provision(js).await.map_err(|e| e.to_string())?;
        let registry = Registry::new(store);

        let cap = AgentCapability::new(&agent_type, ["planning"], "actors.list.>");
        registry.register(&cap).await.map_err(|e| e.to_string())?;

        let all = registry.list_all().await.map_err(|e| e.to_string())?;
        if !all.iter().any(|a| a.agent_type == agent_type) {
            return Err(format!("{agent_type} not found in list_all result (got {} entries)", all.len()));
        }
        Ok(())
    }.await;

    match result { Ok(()) => { ok(LABEL); true } Err(e) => { ko(LABEL, &e); false } }
}

async fn test_registry_discover_by_capability(js: &jetstream::Context) -> bool {
    const LABEL: &str = "Registry — discover finds agents by capability";
    let id = uid();
    let cap_name = format!("capability-{id}");
    let agent_type = format!("DiscoverActor-{id}");

    let result: Result<(), String> = async {
        let store = provision(js).await.map_err(|e| e.to_string())?;
        let registry = Registry::new(store);

        let cap = AgentCapability::new(&agent_type, [cap_name.as_str()], "actors.discover.>");
        registry.register(&cap).await.map_err(|e| e.to_string())?;

        let found = registry.discover(&cap_name).await.map_err(|e| e.to_string())?;
        if !found.iter().any(|a| a.agent_type == agent_type) {
            return Err(format!("{agent_type} not returned by discover({cap_name})"));
        }
        Ok(())
    }.await;

    match result { Ok(()) => { ok(LABEL); true } Err(e) => { ko(LABEL, &e); false } }
}

async fn test_registry_unregister(js: &jetstream::Context) -> bool {
    const LABEL: &str = "Registry — unregister removes agent from get + list_all";
    let id = uid();
    let agent_type = format!("UnregActor-{id}");

    let result: Result<(), String> = async {
        let store = provision(js).await.map_err(|e| e.to_string())?;
        let registry = Registry::new(store);

        let cap = AgentCapability::new(&agent_type, ["temp"], "actors.unreg.>");
        registry.register(&cap).await.map_err(|e| e.to_string())?;

        // Confirm registered
        let found = registry.get(&agent_type).await.map_err(|e| e.to_string())?;
        if found.is_none() { return Err("not found after registration".into()); }

        // Unregister
        registry.unregister(&agent_type).await.map_err(|e| e.to_string())?;

        // Confirm removed
        let after = registry.get(&agent_type).await.map_err(|e| e.to_string())?;
        if after.is_some() { return Err("agent still present after unregister".into()); }
        Ok(())
    }.await;

    match result { Ok(()) => { ok(LABEL); true } Err(e) => { ko(LABEL, &e); false } }
}

async fn test_registry_get_nonexistent(js: &jetstream::Context) -> bool {
    const LABEL: &str = "Registry — get of unknown agent_type returns None";

    let result: Result<(), String> = async {
        let store = provision(js).await.map_err(|e| e.to_string())?;
        let registry = Registry::new(store);
        let found = registry.get("nonexistent-actor-xyzzy").await.map_err(|e| e.to_string())?;
        if found.is_some() { return Err("expected None, got Some".into()); }
        Ok(())
    }.await;

    match result { Ok(()) => { ok(LABEL); true } Err(e) => { ko(LABEL, &e); false } }
}

async fn test_registry_refresh_keeps_alive(js: &jetstream::Context) -> bool {
    const LABEL: &str = "Registry — refresh re-publishes without changing agent_type";
    let id = uid();
    let agent_type = format!("RefreshActor-{id}");

    let result: Result<(), String> = async {
        let store = provision(js).await.map_err(|e| e.to_string())?;
        let registry = Registry::new(store);

        let mut cap = AgentCapability::new(&agent_type, ["refresh_test"], "actors.refresh.>");
        registry.register(&cap).await.map_err(|e| e.to_string())?;

        // Update load and refresh
        cap.current_load = 5;
        registry.refresh(&cap).await.map_err(|e| e.to_string())?;

        let found = registry.get(&agent_type).await.map_err(|e| e.to_string())?.ok_or("not found")?;
        if found.current_load != 5 { return Err(format!("expected load 5, got {}", found.current_load)); }
        Ok(())
    }.await;

    match result { Ok(()) => { ok(LABEL); true } Err(e) => { ko(LABEL, &e); false } }
}

async fn test_registry_find_by_model(js: &jetstream::Context) -> bool {
    const LABEL: &str = "Registry — find_by_model returns agent matching model metadata";
    let id = uid();
    let agent_type = format!("ModelActor-{id}");
    let model_id = format!("grok-test-{id}");

    let result: Result<(), String> = async {
        let store = provision(js).await.map_err(|e| e.to_string())?;
        let registry = Registry::new(store);

        let mut cap = AgentCapability::new(&agent_type, ["model_test"], "actors.model.>");
        cap.metadata = serde_json::json!({"models": [model_id]});
        registry.register(&cap).await.map_err(|e| e.to_string())?;

        let found = registry.find_by_model(&model_id).await.map_err(|e| e.to_string())?;
        let found = found.ok_or_else(|| format!("agent with model_id={model_id} not found"))?;
        if found.agent_type != agent_type { return Err(format!("agent_type mismatch: {}", found.agent_type)); }
        Ok(())
    }.await;

    match result { Ok(()) => { ok(LABEL); true } Err(e) => { ko(LABEL, &e); false } }
}

// ── Transcript tests ───────────────────────────────────────────────────────────

async fn test_transcript_write_and_query(js: &jetstream::Context) -> bool {
    const LABEL: &str = "Transcript — write user+assistant messages, query by entity";
    let id = uid();
    let actor_key = format!("owner/repo/{id}");

    let result: Result<(), String> = async {
        let store = TranscriptStore::new(js.clone());
        store.provision().await.map_err(|e| e.to_string())?;

        let publisher = NatsTranscriptPublisher::new(js.clone());
        let session = Session::new(publisher, "pr", &actor_key);

        session.append_user_message("Review this PR.", None).await.map_err(|e| e.to_string())?;
        session.append_assistant_message("LGTM, ship it.", Some(42)).await.map_err(|e| e.to_string())?;

        // Wait briefly for JetStream to settle
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        let entries = store.query("pr", &actor_key).await.map_err(|e| e.to_string())?;
        if entries.len() != 2 {
            return Err(format!("expected 2 entries, got {}", entries.len()));
        }

        let user_entry = &entries[0];
        if !matches!(user_entry, trogon_transcript::TranscriptEntry::Message { role: Role::User, .. }) {
            return Err(format!("first entry should be User message, got: {user_entry:?}"));
        }

        let asst_entry = &entries[1];
        if !matches!(asst_entry, trogon_transcript::TranscriptEntry::Message { role: Role::Assistant, tokens: Some(42), .. }) {
            return Err(format!("second entry should be Assistant message with 42 tokens, got: {asst_entry:?}"));
        }
        Ok(())
    }.await;

    match result { Ok(()) => { ok(LABEL); true } Err(e) => { ko(LABEL, &e); false } }
}

async fn test_transcript_replay_single_session(js: &jetstream::Context) -> bool {
    const LABEL: &str = "Transcript — replay returns only entries for the requested session_id";
    let id = uid();
    let actor_key = format!("owner/repo/{id}");

    let result: Result<(), String> = async {
        let store = TranscriptStore::new(js.clone());
        store.provision().await.map_err(|e| e.to_string())?;

        let publisher = NatsTranscriptPublisher::new(js.clone());

        // Session A
        let session_a = Session::new(publisher.clone(), "incident", &actor_key);
        session_a.append_user_message("session A message", None).await.map_err(|e| e.to_string())?;

        // Session B (different session_id)
        let session_b = Session::new(publisher, "incident", &actor_key);
        session_b.append_user_message("session B message", None).await.map_err(|e| e.to_string())?;
        session_b.append_assistant_message("session B reply", None).await.map_err(|e| e.to_string())?;

        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        // Replay session B only
        let entries = store.replay("incident", &actor_key, session_b.id()).await.map_err(|e| e.to_string())?;
        if entries.len() != 2 {
            return Err(format!("expected 2 entries for session B, got {}", entries.len()));
        }

        // Replay session A only
        let entries_a = store.replay("incident", &actor_key, session_a.id()).await.map_err(|e| e.to_string())?;
        if entries_a.len() != 1 {
            return Err(format!("expected 1 entry for session A, got {}", entries_a.len()));
        }

        // Full query should have 3 entries total
        let all = store.query("incident", &actor_key).await.map_err(|e| e.to_string())?;
        if all.len() != 3 {
            return Err(format!("expected 3 total entries across both sessions, got {}", all.len()));
        }
        Ok(())
    }.await;

    match result { Ok(()) => { ok(LABEL); true } Err(e) => { ko(LABEL, &e); false } }
}

async fn test_transcript_tool_call_entry(js: &jetstream::Context) -> bool {
    const LABEL: &str = "Transcript — append_tool_call writes ToolCall entry";
    let id = uid();
    let actor_key = format!("tool-entity-{id}");

    let result: Result<(), String> = async {
        let store = TranscriptStore::new(js.clone());
        store.provision().await.map_err(|e| e.to_string())?;

        let publisher = NatsTranscriptPublisher::new(js.clone());
        let session = Session::new(publisher, "tool_actor", &actor_key);

        session.append_tool_call(
            "bash",
            serde_json::json!({"command": "ls"}),
            serde_json::json!("file1.rs\nfile2.rs"),
            12,
        ).await.map_err(|e| e.to_string())?;

        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        let entries = store.query("tool_actor", &actor_key).await.map_err(|e| e.to_string())?;
        if entries.len() != 1 { return Err(format!("expected 1 entry, got {}", entries.len())); }

        if !matches!(&entries[0], trogon_transcript::TranscriptEntry::ToolCall { name, .. } if name == "bash") {
            return Err(format!("expected ToolCall for 'bash', got {:?}", entries[0]));
        }
        Ok(())
    }.await;

    match result { Ok(()) => { ok(LABEL); true } Err(e) => { ko(LABEL, &e); false } }
}

async fn test_transcript_query_empty_entity(js: &jetstream::Context) -> bool {
    const LABEL: &str = "Transcript — query of unknown entity returns empty vec";

    let result: Result<(), String> = async {
        let store = TranscriptStore::new(js.clone());
        store.provision().await.map_err(|e| e.to_string())?;

        let entries = store.query("ghost_actor", "nonexistent/entity/999").await.map_err(|e| e.to_string())?;
        if !entries.is_empty() {
            return Err(format!("expected empty result, got {} entries", entries.len()));
        }
        Ok(())
    }.await;

    match result { Ok(()) => { ok(LABEL); true } Err(e) => { ko(LABEL, &e); false } }
}

// ══════════════════════════════════════════════════════════════════════════════
// Entry point
// ══════════════════════════════════════════════════════════════════════════════

#[tokio::main]
async fn main() {
    println!();
    println!("══════════════════════════════════════════════════════════");
    println!(" trogon-registry + trogon-transcript — live test");
    println!("  NATS: nats://localhost:4222  (JetStream required)");
    println!("══════════════════════════════════════════════════════════");
    println!();

    let nats = async_nats::connect("nats://localhost:4222")
        .await
        .expect("NATS must be running on localhost:4222 — start it with: nats-server -js");
    let js = jetstream::new(nats);

    println!("Registry");
    let r1 = test_registry_register_and_get(&js).await;
    let r2 = test_registry_list_all(&js).await;
    let r3 = test_registry_discover_by_capability(&js).await;
    let r4 = test_registry_unregister(&js).await;
    let r5 = test_registry_get_nonexistent(&js).await;
    let r6 = test_registry_refresh_keeps_alive(&js).await;
    let r7 = test_registry_find_by_model(&js).await;

    println!();
    println!("Transcript");
    let r8 = test_transcript_write_and_query(&js).await;
    let r9 = test_transcript_replay_single_session(&js).await;
    let r10 = test_transcript_tool_call_entry(&js).await;
    let r11 = test_transcript_query_empty_entity(&js).await;

    let results = [r1, r2, r3, r4, r5, r6, r7, r8, r9, r10, r11];
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

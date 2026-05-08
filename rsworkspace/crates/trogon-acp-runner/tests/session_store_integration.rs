//! Integration tests for `SessionStore` — requires Docker (testcontainers starts a NATS server).
//!
//! Run with:
//!   cargo test -p trogon-acp-runner --test session_store_integration

use async_nats::jetstream;
use testcontainers_modules::nats::Nats;
use testcontainers_modules::testcontainers::runners::AsyncRunner;
use testcontainers_modules::testcontainers::{ContainerAsync, ImageExt};
use trogon_acp_runner::session_store::{AuditEntry, AuditOutcome, PolicyAction, ToolPolicy};
use trogon_acp_runner::{EgressPolicy, NatsSessionStore, SessionState, SessionStore};
use trogon_acp_runner::egress::{EgressAction, EgressRule};

async fn setup() -> (ContainerAsync<Nats>, async_nats::Client, jetstream::Context) {
    let container: ContainerAsync<Nats> = Nats::default()
        .with_cmd(["--jetstream"])
        .start()
        .await
        .expect("Failed to start NATS container — is Docker running?");
    let port = container.get_host_port_ipv4(4222).await.unwrap();
    let nats = async_nats::connect(format!("127.0.0.1:{port}"))
        .await
        .expect("failed to connect to NATS");
    let js = jetstream::new(nats.clone());
    (container, nats, js)
}

// ── load ─────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn load_missing_session_returns_default() {
    let (_c, _nats, js) = setup().await;
    let store = NatsSessionStore::open(&js).await.unwrap();

    let state = store.load("does-not-exist").await.unwrap();
    assert!(state.messages.is_empty());
    assert_eq!(state.mode, "");
    assert!(state.model.is_none());
}

// ── save + load ───────────────────────────────────────────────────────────────

#[tokio::test]
async fn save_and_load_roundtrip() {
    let (_c, _nats, js) = setup().await;
    let store = NatsSessionStore::open(&js).await.unwrap();

    let state = SessionState {
        mode: "plan".to_string(),
        cwd: "/home/user/project".to_string(),
        title: "My session".to_string(),
        created_at: "2024-01-01T00:00:00Z".to_string(),
        ..Default::default()
    };
    store.save("sess-1", &state).await.unwrap();

    let loaded = store.load("sess-1").await.unwrap();
    assert_eq!(loaded.mode, "plan");
    assert_eq!(loaded.cwd, "/home/user/project");
    assert_eq!(loaded.title, "My session");
}

#[tokio::test]
async fn save_preserves_model_override() {
    let (_c, _nats, js) = setup().await;
    let store = NatsSessionStore::open(&js).await.unwrap();

    let state = SessionState {
        model: Some("claude-sonnet-4-6".to_string()),
        ..Default::default()
    };
    store.save("sess-model", &state).await.unwrap();

    let loaded = store.load("sess-model").await.unwrap();
    assert_eq!(loaded.model.as_deref(), Some("claude-sonnet-4-6"));
}

#[tokio::test]
async fn save_preserves_allowed_tools() {
    let (_c, _nats, js) = setup().await;
    let store = NatsSessionStore::open(&js).await.unwrap();

    let state = SessionState {
        allowed_tools: vec!["Bash".to_string(), "Read".to_string()],
        ..Default::default()
    };
    store.save("sess-tools", &state).await.unwrap();

    let loaded = store.load("sess-tools").await.unwrap();
    assert_eq!(loaded.allowed_tools, vec!["Bash", "Read"]);
}

#[tokio::test]
async fn overwrite_save_updates_value() {
    let (_c, _nats, js) = setup().await;
    let store = NatsSessionStore::open(&js).await.unwrap();

    let v1 = SessionState {
        mode: "default".to_string(),
        ..Default::default()
    };
    store.save("sess-rw", &v1).await.unwrap();

    let v2 = SessionState {
        mode: "plan".to_string(),
        ..Default::default()
    };
    store.save("sess-rw", &v2).await.unwrap();

    let loaded = store.load("sess-rw").await.unwrap();
    assert_eq!(loaded.mode, "plan");
}

// ── delete ────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn delete_removes_session() {
    let (_c, _nats, js) = setup().await;
    let store = NatsSessionStore::open(&js).await.unwrap();

    let state = SessionState {
        mode: "default".to_string(),
        ..Default::default()
    };
    store.save("sess-del", &state).await.unwrap();
    store.delete("sess-del").await.unwrap();

    // After deletion, loading returns the empty default
    let loaded = store.load("sess-del").await.unwrap();
    assert_eq!(loaded.mode, "");
}

#[tokio::test]
async fn delete_nonexistent_does_not_error() {
    let (_c, _nats, js) = setup().await;
    let store = NatsSessionStore::open(&js).await.unwrap();
    // Should not panic or return Err
    let result = store.delete("never-existed").await;
    assert!(result.is_ok());
}

// ── list_ids ──────────────────────────────────────────────────────────────────

#[tokio::test]
async fn list_ids_empty_store_returns_empty() {
    let (_c, _nats, js) = setup().await;
    let store = NatsSessionStore::open(&js).await.unwrap();
    let ids = store.list_ids().await.unwrap();
    assert!(ids.is_empty(), "new store must have no sessions");
}

#[tokio::test]
async fn list_ids_returns_all_saved_sessions() {
    let (_c, _nats, js) = setup().await;
    let store = NatsSessionStore::open(&js).await.unwrap();

    for id in &["alpha", "beta", "gamma"] {
        store
            .save(
                id,
                &SessionState {
                    mode: "default".to_string(),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
    }

    let mut ids = store.list_ids().await.unwrap();
    ids.sort();
    assert_eq!(ids, vec!["alpha", "beta", "gamma"]);
}

#[tokio::test]
async fn list_ids_excludes_deleted_session() {
    let (_c, _nats, js) = setup().await;
    let store = NatsSessionStore::open(&js).await.unwrap();

    store
        .save(
            "keep",
            &SessionState {
                mode: "default".to_string(),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    store
        .save(
            "drop",
            &SessionState {
                mode: "default".to_string(),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    store.delete("drop").await.unwrap();

    let ids = store.list_ids().await.unwrap();
    assert!(ids.contains(&"keep".to_string()));
    assert!(!ids.contains(&"drop".to_string()));
}

// ── corrupted data ────────────────────────────────────────────────────────────

/// If the KV bucket contains raw bytes that are not valid JSON for SessionState,
/// `load()` must return an error (not panic, not silently return default).
#[tokio::test]
async fn load_corrupted_json_returns_error() {
    let (_c, _, js) = setup().await;
    let store = NatsSessionStore::open(&js).await.unwrap();

    // Write raw invalid JSON directly to the underlying KV bucket.
    let kv = js
        .create_key_value(async_nats::jetstream::kv::Config {
            bucket: "ACP_SESSIONS".to_string(),
            ..Default::default()
        })
        .await
        .unwrap();
    kv.put(
        "sess-corrupt-1",
        bytes::Bytes::from(b"not valid json at all".to_vec()),
    )
    .await
    .unwrap();

    let result = store.load("sess-corrupt-1").await;
    assert!(
        result.is_err(),
        "loading corrupted session data must return an error"
    );
    let err_msg = result.unwrap_err().to_string();
    assert!(
        !err_msg.is_empty(),
        "error must contain a meaningful message, got empty string"
    );
}

/// If the KV bucket contains an empty byte array, `load()` must return an error.
#[tokio::test]
async fn load_empty_bytes_returns_error() {
    let (_c, _, js) = setup().await;
    let store = NatsSessionStore::open(&js).await.unwrap();

    let kv = js
        .create_key_value(async_nats::jetstream::kv::Config {
            bucket: "ACP_SESSIONS".to_string(),
            ..Default::default()
        })
        .await
        .unwrap();
    kv.put("sess-empty-1", bytes::Bytes::new()).await.unwrap();

    let result = store.load("sess-empty-1").await;
    assert!(
        result.is_err(),
        "loading empty session bytes must return an error"
    );
}

/// If the KV bucket contains valid JSON but for a completely different type,
/// `load()` must return an error.
#[tokio::test]
async fn load_wrong_json_type_returns_error() {
    let (_c, _, js) = setup().await;
    let store = NatsSessionStore::open(&js).await.unwrap();

    let kv = js
        .create_key_value(async_nats::jetstream::kv::Config {
            bucket: "ACP_SESSIONS".to_string(),
            ..Default::default()
        })
        .await
        .unwrap();
    // Valid JSON but not a SessionState object — it's a string
    kv.put(
        "sess-wrong-1",
        bytes::Bytes::from(b"\"just a string\"".to_vec()),
    )
    .await
    .unwrap();

    let result = store.load("sess-wrong-1").await;
    assert!(
        result.is_err(),
        "loading wrong JSON type must return an error"
    );
}

// ── new fields: tool_policies, egress_policy, audit_log ──────────────────────

#[tokio::test]
async fn save_and_load_tool_policies() {
    let (_c, _nats, js) = setup().await;
    let store = NatsSessionStore::open(&js).await.unwrap();

    let state = SessionState {
        tool_policies: vec![ToolPolicy {
            tool: "write_file".to_string(),
            path_pattern: "/workspace/**".to_string(),
            action: PolicyAction::Allow,
        }],
        ..Default::default()
    };
    store.save("sess-tp", &state).await.unwrap();

    let loaded = store.load("sess-tp").await.unwrap();
    assert_eq!(loaded.tool_policies.len(), 1);
    assert_eq!(loaded.tool_policies[0].tool, "write_file");
    assert!(matches!(loaded.tool_policies[0].action, PolicyAction::Allow));
}

#[tokio::test]
async fn save_and_load_egress_policy() {
    let (_c, _nats, js) = setup().await;
    let store = NatsSessionStore::open(&js).await.unwrap();

    let state = SessionState {
        egress_policy: Some(EgressPolicy {
            default_action: EgressAction::Deny,
            rules: vec![EgressRule {
                host_pattern: "api.anthropic.com".to_string(),
                action: EgressAction::Allow,
            }],
        }),
        ..Default::default()
    };
    store.save("sess-egress", &state).await.unwrap();

    let loaded = store.load("sess-egress").await.unwrap();
    let policy = loaded.egress_policy.unwrap();
    assert!(matches!(policy.default_action, EgressAction::Deny));
    assert_eq!(policy.rules[0].host_pattern, "api.anthropic.com");
}

#[tokio::test]
async fn save_and_load_audit_log() {
    let (_c, _nats, js) = setup().await;
    let store = NatsSessionStore::open(&js).await.unwrap();

    let state = SessionState {
        audit_log: vec![
            AuditEntry {
                timestamp: "2026-01-01T00:00:00Z".to_string(),
                tool: "Read".to_string(),
                input_summary: "/etc/hosts".to_string(),
                outcome: AuditOutcome::Allowed,
            },
            AuditEntry {
                timestamp: "2026-01-01T00:00:01Z".to_string(),
                tool: "Bash".to_string(),
                input_summary: "cargo build".to_string(),
                outcome: AuditOutcome::ApprovedByUser,
            },
        ],
        ..Default::default()
    };
    store.save("sess-audit", &state).await.unwrap();

    let loaded = store.load("sess-audit").await.unwrap();
    assert_eq!(loaded.audit_log.len(), 2);
    assert_eq!(loaded.audit_log[0].tool, "Read");
    assert_eq!(loaded.audit_log[0].outcome, AuditOutcome::Allowed);
    assert_eq!(loaded.audit_log[1].tool, "Bash");
    assert_eq!(loaded.audit_log[1].outcome, AuditOutcome::ApprovedByUser);
}

#[tokio::test]
async fn egress_policy_none_not_stored_in_json() {
    let (_c, _nats, js) = setup().await;
    let store = NatsSessionStore::open(&js).await.unwrap();

    let state = SessionState::default();
    store.save("sess-no-egress", &state).await.unwrap();

    let loaded = store.load("sess-no-egress").await.unwrap();
    assert!(loaded.egress_policy.is_none());
}

// ── open idempotency ──────────────────────────────────────────────────────────

#[tokio::test]
async fn open_twice_is_idempotent() {
    let (_c, _nats, js) = setup().await;
    let store1 = NatsSessionStore::open(&js).await.unwrap();
    let store2 = NatsSessionStore::open(&js).await.unwrap();

    store1
        .save(
            "s1",
            &SessionState {
                mode: "plan".to_string(),
                ..Default::default()
            },
        )
        .await
        .unwrap();

    // Both handles share the same KV bucket
    let loaded = store2.load("s1").await.unwrap();
    assert_eq!(loaded.mode, "plan");
}

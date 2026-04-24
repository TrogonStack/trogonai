//! Integration tests for session branching against a real NATS JetStream KV store.
//!
//! Four tests cover the gaps not exercised by agent_unit.rs (mock store):
//!
//!  1. `NatsSessionStore::list_children` — directly saves session states with
//!     `parent_session_id` set and verifies the KV scan returns the right IDs.
//!
//!  2. `fork_session` persists `parent_session_id` and `branched_at_index` —
//!     drives `TrogonAgent::fork_session` with a real `NatsSessionStore` and
//!     reads the written KV entry to confirm both fields are present.
//!
//!  3. `ext_method("session/list_children")` end-to-end with real store —
//!     forks two direct children and one grandchild, then verifies the agent
//!     only returns the direct children.
//!
//!  4. `list_sessions` includes `parentSessionId` in `_meta` with real store —
//!     forks a session, calls `list_sessions`, and verifies the response reads
//!     `parent_session_id` back from KV and exposes it in the `_meta` field.
//!
//! Requires Docker (uses testcontainers to spin up a NATS JetStream server).
//!
//! Run with:
//!   cargo test -p trogon-acp-runner --test session_branching_nats --features test-helpers

use std::sync::Arc;

use agent_client_protocol::{
    Agent, ExtRequest, ForkSessionRequest, InitializeRequest, ListSessionsRequest,
    NewSessionRequest, ProtocolVersion,
};
use async_nats::jetstream;
use testcontainers_modules::nats::Nats;
use testcontainers_modules::testcontainers::{ContainerAsync, ImageExt, runners::AsyncRunner};
use tokio::sync::RwLock;
use trogon_acp_runner::{
    GatewayConfig, NatsSessionStore, SessionState, SessionStore, TrogonAgent,
    agent_runner::mock::MockAgentRunner,
    session_notifier::mock::MockSessionNotifier,
};
use trogon_agent_core::agent_loop::Message;

// ── helpers ───────────────────────────────────────────────────────────────────

async fn start_nats() -> (ContainerAsync<Nats>, u16) {
    let c = Nats::default()
        .with_cmd(["--jetstream"])
        .start()
        .await
        .expect("Failed to start NATS container — is Docker running?");
    let port = c.get_host_port_ipv4(4222).await.unwrap();
    (c, port)
}

async fn make_js(port: u16) -> (async_nats::Client, jetstream::Context) {
    let nats = async_nats::connect(format!("127.0.0.1:{port}"))
        .await
        .expect("Failed to connect to NATS");
    let js = jetstream::new(nats.clone());
    (nats, js)
}

type TestAgent = TrogonAgent<NatsSessionStore, MockAgentRunner, MockSessionNotifier>;

fn make_agent(store: NatsSessionStore) -> TestAgent {
    TrogonAgent::new(
        MockSessionNotifier::new(),
        store,
        MockAgentRunner::new("claude-test"),
        "acp",
        "claude-test",
        None,
        Arc::new(RwLock::new(None::<GatewayConfig>)),
    )
}

// ── tests ─────────────────────────────────────────────────────────────────────

/// `NatsSessionStore::list_children` scans the KV bucket and returns only the
/// sessions whose `parent_session_id` matches the requested parent.
#[tokio::test]
async fn nats_session_store_list_children_returns_children() {
    let (_c, port) = start_nats().await;
    let (_, js) = make_js(port).await;

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let store = NatsSessionStore::open(&js).await.unwrap();

            let parent_id = "parent-1";
            let child1 = "child-a";
            let child2 = "child-b";
            let unrelated = "unrelated";

            // Save parent and unrelated with no parent_session_id.
            store.save(parent_id, &SessionState::default()).await.unwrap();
            store.save(unrelated, &SessionState::default()).await.unwrap();

            // Save two direct children.
            store
                .save(child1, &SessionState { parent_session_id: Some(parent_id.to_string()), ..Default::default() })
                .await
                .unwrap();
            store
                .save(child2, &SessionState { parent_session_id: Some(parent_id.to_string()), ..Default::default() })
                .await
                .unwrap();

            let mut children = store.list_children(parent_id).await.unwrap();
            children.sort();
            assert_eq!(
                children,
                vec![child1.to_string(), child2.to_string()],
                "list_children must return exactly the two direct children"
            );

            // Unrelated session must not appear.
            let children_of_unrelated = store.list_children(unrelated).await.unwrap();
            assert!(children_of_unrelated.is_empty(), "unrelated session must have no children");
        })
        .await;
}

/// `TrogonAgent::fork_session` writes `parent_session_id` and
/// `branched_at_index` into the NATS KV bucket so the fields survive a
/// round-trip through the real store.
#[tokio::test]
async fn fork_session_persists_parent_and_branch_index_to_nats_kv() {
    let (_c, port) = start_nats().await;
    let (_, js) = make_js(port).await;

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let store = NatsSessionStore::open(&js).await.unwrap();
            let agent = make_agent(store.clone());

            let parent_id = agent
                .new_session(NewSessionRequest::new("/root"))
                .await
                .unwrap()
                .session_id
                .to_string();

            let meta = serde_json::from_value::<serde_json::Map<String, serde_json::Value>>(
                serde_json::json!({ "branchAtIndex": 3 }),
            )
            .unwrap();
            let fork_id = agent
                .fork_session(
                    ForkSessionRequest::new(parent_id.clone(), "/fork").meta(meta),
                )
                .await
                .unwrap()
                .session_id
                .to_string();

            let fork_state = store.load(&fork_id).await.unwrap();
            assert_eq!(
                fork_state.parent_session_id.as_deref(),
                Some(parent_id.as_str()),
                "fork KV entry must record parent_session_id"
            );
            assert_eq!(
                fork_state.branched_at_index,
                Some(3),
                "fork KV entry must record branched_at_index"
            );
        })
        .await;
}

/// `ext_method("session/list_children")` drives `NatsSessionStore::list_children`
/// under the hood. It must return only the direct children, not grandchildren.
#[tokio::test]
async fn ext_list_children_returns_direct_children_from_real_store() {
    let (_c, port) = start_nats().await;
    let (_, js) = make_js(port).await;

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let store = NatsSessionStore::open(&js).await.unwrap();
            let agent = make_agent(store);

            let parent_id = agent
                .new_session(NewSessionRequest::new("/root"))
                .await
                .unwrap()
                .session_id
                .to_string();

            let child1 = agent
                .fork_session(ForkSessionRequest::new(parent_id.clone(), "/c1"))
                .await
                .unwrap()
                .session_id
                .to_string();

            let child2 = agent
                .fork_session(ForkSessionRequest::new(parent_id.clone(), "/c2"))
                .await
                .unwrap()
                .session_id
                .to_string();

            // Grandchild — must NOT appear in parent's list.
            let _grandchild = agent
                .fork_session(ForkSessionRequest::new(child1.clone(), "/gc"))
                .await
                .unwrap()
                .session_id
                .to_string();

            let params_json = format!(r#"{{"sessionId":"{}"}}"#, parent_id);
            let params: Arc<serde_json::value::RawValue> =
                serde_json::value::RawValue::from_string(params_json).unwrap().into();
            let resp = agent
                .ext_method(ExtRequest::new("session/list_children", params))
                .await
                .unwrap();

            let v: serde_json::Value = serde_json::from_str(resp.0.get()).unwrap();
            let mut children: Vec<String> = v["children"]
                .as_array()
                .expect("response must contain a children array")
                .iter()
                .filter_map(|x| x.as_str().map(String::from))
                .collect();
            children.sort();

            let mut expected = vec![child1, child2];
            expected.sort();
            assert_eq!(children, expected, "ext_method must return only direct children");
        })
        .await;
}

/// `initialize` advertises both `listChildren` and `branchAtIndex` in the
/// `session_capabilities._meta` map when backed by a real NATS store.
#[tokio::test]
async fn initialize_advertises_listchildren_and_branchatindex_capabilities() {
    let (_c, port) = start_nats().await;
    let (_, js) = make_js(port).await;

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let store = NatsSessionStore::open(&js).await.unwrap();
            let agent = make_agent(store);

            let resp = agent
                .initialize(InitializeRequest::new(ProtocolVersion::LATEST))
                .await
                .unwrap();

            let meta = resp
                .agent_capabilities
                .session_capabilities
                .meta
                .expect("session_capabilities must have _meta");
            assert!(meta.contains_key("listChildren"), "must advertise listChildren");
            assert!(meta.contains_key("branchAtIndex"), "must advertise branchAtIndex");
        })
        .await;
}

/// `branchAtIndex: 0` is persisted to NATS KV as `branched_at_index: Some(0)`.
#[tokio::test]
async fn fork_session_branch_at_index_zero_persists_to_nats_kv() {
    let (_c, port) = start_nats().await;
    let (_, js) = make_js(port).await;

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let store = NatsSessionStore::open(&js).await.unwrap();
            let agent = make_agent(store.clone());

            let parent_id = agent
                .new_session(NewSessionRequest::new("/root"))
                .await
                .unwrap()
                .session_id
                .to_string();

            let meta = serde_json::from_value::<serde_json::Map<String, serde_json::Value>>(
                serde_json::json!({ "branchAtIndex": 0 }),
            )
            .unwrap();
            let fork_id = agent
                .fork_session(ForkSessionRequest::new(parent_id, "/fork").meta(meta))
                .await
                .unwrap()
                .session_id
                .to_string();

            let fork_state = store.load(&fork_id).await.unwrap();
            assert_eq!(
                fork_state.branched_at_index,
                Some(0),
                "branchAtIndex: 0 must be persisted as Some(0) in NATS KV"
            );
        })
        .await;
}

/// `branchAtIndex: 99` (out-of-bounds for an empty history) is persisted to
/// NATS KV as `branched_at_index: Some(99)`.
#[tokio::test]
async fn fork_session_branch_at_index_out_of_bounds_persists_to_nats_kv() {
    let (_c, port) = start_nats().await;
    let (_, js) = make_js(port).await;

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let store = NatsSessionStore::open(&js).await.unwrap();
            let agent = make_agent(store.clone());

            let parent_id = agent
                .new_session(NewSessionRequest::new("/root"))
                .await
                .unwrap()
                .session_id
                .to_string();

            let meta = serde_json::from_value::<serde_json::Map<String, serde_json::Value>>(
                serde_json::json!({ "branchAtIndex": 99 }),
            )
            .unwrap();
            let fork_id = agent
                .fork_session(ForkSessionRequest::new(parent_id, "/fork").meta(meta))
                .await
                .unwrap()
                .session_id
                .to_string();

            let fork_state = store.load(&fork_id).await.unwrap();
            assert_eq!(
                fork_state.branched_at_index,
                Some(99),
                "out-of-bounds branchAtIndex must be persisted as Some(99) in NATS KV"
            );
        })
        .await;
}

/// `ext_method("session/list_children")` with a missing `sessionId` field
/// treats the parent as an empty string and returns an empty children array.
#[tokio::test]
async fn ext_list_children_missing_session_id_returns_empty_children() {
    let (_c, port) = start_nats().await;
    let (_, js) = make_js(port).await;

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let store = NatsSessionStore::open(&js).await.unwrap();
            let agent = make_agent(store);

            let params: Arc<serde_json::value::RawValue> =
                serde_json::value::RawValue::from_string("{}".to_string()).unwrap().into();
            let resp = agent
                .ext_method(ExtRequest::new("session/list_children", params))
                .await
                .unwrap();

            let v: serde_json::Value = serde_json::from_str(resp.0.get()).unwrap();
            assert_eq!(
                v["children"].as_array().map(Vec::len),
                Some(0),
                "missing sessionId must yield an empty children array"
            );
        })
        .await;
}

/// `ext_method("session/list_children")` with a wrong-type `sessionId`
/// (integer instead of string) returns an empty children array.
#[tokio::test]
async fn ext_list_children_wrong_type_session_id_returns_empty_children() {
    let (_c, port) = start_nats().await;
    let (_, js) = make_js(port).await;

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let store = NatsSessionStore::open(&js).await.unwrap();
            let agent = make_agent(store);

            let params: Arc<serde_json::value::RawValue> =
                serde_json::value::RawValue::from_string(r#"{"sessionId":123}"#.to_string())
                    .unwrap()
                    .into();
            let resp = agent
                .ext_method(ExtRequest::new("session/list_children", params))
                .await
                .unwrap();

            let v: serde_json::Value = serde_json::from_str(resp.0.get()).unwrap();
            assert_eq!(
                v["children"].as_array().map(Vec::len),
                Some(0),
                "wrong-type sessionId must yield an empty children array"
            );
        })
        .await;
}

/// `list_sessions` reads `branched_at_index` back from the real NATS KV bucket
/// and exposes it as `branchedAtIndex` in the session `_meta`. The root
/// session must not have `branchedAtIndex` in its `_meta`.
#[tokio::test]
async fn list_sessions_includes_branched_at_index_in_meta_with_real_store() {
    let (_c, port) = start_nats().await;
    let (_, js) = make_js(port).await;

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let store = NatsSessionStore::open(&js).await.unwrap();
            let agent = make_agent(store);

            let parent_id = agent
                .new_session(NewSessionRequest::new("/root"))
                .await
                .unwrap()
                .session_id
                .to_string();

            let meta = serde_json::from_value::<serde_json::Map<String, serde_json::Value>>(
                serde_json::json!({ "branchAtIndex": 2 }),
            )
            .unwrap();
            let fork_id = agent
                .fork_session(ForkSessionRequest::new(parent_id.clone(), "/fork").meta(meta))
                .await
                .unwrap()
                .session_id
                .to_string();

            let list_resp = agent.list_sessions(ListSessionsRequest::new()).await.unwrap();

            let fork_info = list_resp
                .sessions
                .iter()
                .find(|s| s.session_id.to_string() == fork_id)
                .expect("forked session must appear in list_sessions");
            let session_meta = fork_info.meta.as_ref().expect("fork must have _meta");
            assert_eq!(
                session_meta.get("branchedAtIndex").and_then(|v| v.as_u64()),
                Some(2),
                "branchedAtIndex must be 2 in fork _meta after KV round-trip"
            );

            let root_info = list_resp
                .sessions
                .iter()
                .find(|s| s.session_id.to_string() == parent_id)
                .expect("root session must appear in list_sessions");
            assert!(
                root_info
                    .meta
                    .as_ref()
                    .map_or(true, |m| !m.contains_key("branchedAtIndex")),
                "root session must not have branchedAtIndex in _meta"
            );
        })
        .await;
}

/// `list_sessions` reads `parent_session_id` back from the real NATS KV bucket
/// and exposes it as `parentSessionId` in the session `_meta`.  The root
/// session must have no `_meta`.
#[tokio::test]
async fn list_sessions_shows_parent_session_id_in_meta_with_real_store() {
    let (_c, port) = start_nats().await;
    let (_, js) = make_js(port).await;

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let store = NatsSessionStore::open(&js).await.unwrap();
            let agent = make_agent(store);

            let parent_id = agent
                .new_session(NewSessionRequest::new("/root"))
                .await
                .unwrap()
                .session_id
                .to_string();

            let fork_id = agent
                .fork_session(ForkSessionRequest::new(parent_id.clone(), "/fork"))
                .await
                .unwrap()
                .session_id
                .to_string();

            let list_resp = agent
                .list_sessions(ListSessionsRequest::new())
                .await
                .unwrap();

            let fork_info = list_resp
                .sessions
                .iter()
                .find(|s| s.session_id.to_string() == fork_id)
                .expect("forked session must appear in list_sessions");
            let meta = fork_info.meta.as_ref().expect("fork must have _meta");
            assert_eq!(
                meta.get("parentSessionId").and_then(|v| v.as_str()),
                Some(parent_id.as_str()),
                "parentSessionId must be present in fork _meta after KV round-trip"
            );

            let root_info = list_resp
                .sessions
                .iter()
                .find(|s| s.session_id.to_string() == parent_id)
                .expect("root session must appear in list_sessions");
            assert!(root_info.meta.is_none(), "root session must not have branch _meta");
        })
        .await;
}

/// When `branchAtIndex` is a JSON string instead of an integer the value is
/// silently ignored: the fork gets a full history copy and `branched_at_index`
/// is `None` in the NATS KV entry.
#[tokio::test]
async fn fork_session_branch_at_index_wrong_type_persists_full_copy_to_nats_kv() {
    let (_c, port) = start_nats().await;
    let (_, js) = make_js(port).await;

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let store = NatsSessionStore::open(&js).await.unwrap();
            let agent = make_agent(store.clone());

            let parent_id = agent
                .new_session(NewSessionRequest::new("/src"))
                .await
                .unwrap()
                .session_id
                .to_string();

            let mut state = store.load(&parent_id).await.unwrap();
            state.messages = (0..4)
                .map(|i| Message::user_text(format!("msg-{i}")))
                .collect();
            store.save(&parent_id, &state).await.unwrap();

            let meta = serde_json::from_value::<serde_json::Map<String, serde_json::Value>>(
                serde_json::json!({ "branchAtIndex": "2" }),
            )
            .unwrap();
            let fork_id = agent
                .fork_session(ForkSessionRequest::new(parent_id, "/branch").meta(meta))
                .await
                .unwrap()
                .session_id
                .to_string();

            let fork_state = store.load(&fork_id).await.unwrap();
            assert_eq!(
                fork_state.messages.len(),
                4,
                "wrong-type branchAtIndex must be ignored — full history must be copied"
            );
            assert_eq!(
                fork_state.branched_at_index,
                None,
                "branched_at_index must be None when branchAtIndex had wrong type"
            );
        })
        .await;
}

/// When the parent session is deleted from NATS KV the fork must still expose
/// the dangling `parentSessionId` in `list_sessions._meta` — the reference is
/// not cleaned up.
#[tokio::test]
async fn list_sessions_preserves_parent_session_id_after_parent_deleted_with_real_store() {
    let (_c, port) = start_nats().await;
    let (_, js) = make_js(port).await;

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let store = NatsSessionStore::open(&js).await.unwrap();
            let agent = make_agent(store.clone());

            let parent_id = agent
                .new_session(NewSessionRequest::new("/root"))
                .await
                .unwrap()
                .session_id
                .to_string();

            let fork_id = agent
                .fork_session(ForkSessionRequest::new(parent_id.clone(), "/fork"))
                .await
                .unwrap()
                .session_id
                .to_string();

            store.delete(&parent_id).await.unwrap();

            let list_resp = agent.list_sessions(ListSessionsRequest::new()).await.unwrap();

            assert!(
                list_resp
                    .sessions
                    .iter()
                    .all(|s| s.session_id.to_string() != parent_id),
                "deleted parent must not appear in list_sessions"
            );

            let fork_info = list_resp
                .sessions
                .iter()
                .find(|s| s.session_id.to_string() == fork_id)
                .expect("fork session must still appear");
            let m = fork_info.meta.as_ref().expect("fork must have _meta");
            assert_eq!(
                m.get("parentSessionId").and_then(|v| v.as_str()),
                Some(parent_id.as_str()),
                "parentSessionId must survive even after parent is deleted from NATS KV"
            );
        })
        .await;
}

/// A fork inherits the parent's `mode` and `model` when those are set.
/// Verifies the values survive a round-trip through NATS KV.
#[tokio::test]
async fn fork_session_inherits_parent_mode_and_model_with_real_store() {
    let (_c, port) = start_nats().await;
    let (_, js) = make_js(port).await;

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let store = NatsSessionStore::open(&js).await.unwrap();
            let agent = make_agent(store.clone());

            let parent_id = agent
                .new_session(NewSessionRequest::new("/src"))
                .await
                .unwrap()
                .session_id
                .to_string();

            let mut state = store.load(&parent_id).await.unwrap();
            state.mode = "plan".to_string();
            state.model = Some("claude-opus-4-7".to_string());
            store.save(&parent_id, &state).await.unwrap();

            let fork_id = agent
                .fork_session(ForkSessionRequest::new(parent_id, "/fork"))
                .await
                .unwrap()
                .session_id
                .to_string();

            let fork_state = store.load(&fork_id).await.unwrap();
            assert_eq!(fork_state.mode, "plan", "fork must inherit parent mode from NATS KV");
            assert_eq!(
                fork_state.model.as_deref(),
                Some("claude-opus-4-7"),
                "fork must inherit parent model from NATS KV"
            );
        })
        .await;
}

/// Forking a session ID that was never created silently succeeds: the store
/// returns `SessionState::default()` for unknown keys, so the fork gets an
/// empty history but still records `parent_session_id`.
#[tokio::test]
async fn fork_session_nonexistent_source_succeeds_with_real_store() {
    let (_c, port) = start_nats().await;
    let (_, js) = make_js(port).await;

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let store = NatsSessionStore::open(&js).await.unwrap();
            let agent = make_agent(store.clone());

            let fork_resp = agent
                .fork_session(ForkSessionRequest::new("ghost-session-id", "/fork"))
                .await;

            assert!(fork_resp.is_ok(), "fork of nonexistent session must succeed");
            let fork_id = fork_resp.unwrap().session_id.to_string();

            let fork_state = store.load(&fork_id).await.unwrap();
            assert_eq!(
                fork_state.parent_session_id.as_deref(),
                Some("ghost-session-id"),
                "fork must record the nonexistent parent session ID in NATS KV"
            );
            assert!(fork_state.messages.is_empty(), "fork of nonexistent session must have empty history");
        })
        .await;
}

/// `ext_method("session/list_children")` with `sessionId: null` treats the
/// parent as an empty string and returns an empty children array (same path
/// as missing sessionId).
#[tokio::test]
async fn ext_list_children_null_session_id_returns_empty_children() {
    let (_c, port) = start_nats().await;
    let (_, js) = make_js(port).await;

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let store = NatsSessionStore::open(&js).await.unwrap();
            let agent = make_agent(store);

            let params: Arc<serde_json::value::RawValue> =
                serde_json::value::RawValue::from_string(r#"{"sessionId":null}"#.to_string())
                    .unwrap()
                    .into();
            let resp = agent
                .ext_method(ExtRequest::new("session/list_children", params))
                .await
                .unwrap();

            let v: serde_json::Value = serde_json::from_str(resp.0.get()).unwrap();
            assert_eq!(
                v["children"].as_array().map(Vec::len),
                Some(0),
                "null sessionId must yield an empty children array"
            );
        })
        .await;
}

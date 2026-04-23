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

use agent_client_protocol::{Agent, ExtRequest, ForkSessionRequest, ListSessionsRequest, NewSessionRequest};
use async_nats::jetstream;
use testcontainers_modules::nats::Nats;
use testcontainers_modules::testcontainers::{ContainerAsync, ImageExt, runners::AsyncRunner};
use tokio::sync::RwLock;
use trogon_acp_runner::{
    GatewayConfig, NatsSessionStore, SessionState, SessionStore, TrogonAgent,
    agent_runner::mock::MockAgentRunner,
    session_notifier::mock::MockSessionNotifier,
};

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

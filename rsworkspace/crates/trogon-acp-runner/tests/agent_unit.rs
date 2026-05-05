//! Unit tests for `TrogonAgent` using in-memory mocks.
//!
//! No Docker, no NATS — all dependencies are replaced by in-process stubs.
//!
//! Run with:
//!   cargo test -p trogon-acp-runner --test agent_unit --features test-helpers

use std::sync::Arc;

use agent_client_protocol::{
    Agent, CancelNotification, CloseSessionRequest, ExtRequest, ForkSessionRequest,
    InitializeRequest, ListSessionsRequest, LoadSessionRequest, NewSessionRequest, PromptRequest,
    ProtocolVersion, ResumeSessionRequest, SetSessionModeRequest, SetSessionModelRequest,
    TextContent,
};
use tokio::sync::RwLock;
use tokio::sync::mpsc;
use trogon_acp_runner::{
    GatewayConfig, SessionStore, TrogonAgent,
    agent_runner::mock::MockAgentRunner,
    elicitation::ElicitationTx,
    session_notifier::mock::MockSessionNotifier,
    session_store::mock::MemorySessionStore,
};
use trogon_agent_core::agent_loop::{AgentError, AgentEvent, ContentBlock as AgentContentBlock, Message};

// ── helpers ───────────────────────────────────────────────────────────────────

fn make_agent() -> TrogonAgent<MemorySessionStore, MockAgentRunner, MockSessionNotifier> {
    TrogonAgent::new(
        MockSessionNotifier::new(),
        MemorySessionStore::new(),
        MockAgentRunner::new("claude-test"),
        "acp",
        "claude-test",
        None,
        None,
        Arc::new(RwLock::new(None::<GatewayConfig>)),
    )
}

fn make_agent_parts() -> (
    MemorySessionStore,
    MockSessionNotifier,
    TrogonAgent<MemorySessionStore, MockAgentRunner, MockSessionNotifier>,
) {
    let store = MemorySessionStore::new();
    let notifier = MockSessionNotifier::new();
    let agent = TrogonAgent::new(
        notifier.clone(),
        store.clone(),
        MockAgentRunner::new("claude-test"),
        "acp",
        "claude-test",
        None,
        None,
        Arc::new(RwLock::new(None::<GatewayConfig>)),
    );
    (store, notifier, agent)
}

// ── initialize ────────────────────────────────────────────────────────────────

#[tokio::test]
async fn initialize_returns_capabilities() {
    let agent = make_agent();
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let resp = agent
                .initialize(InitializeRequest::new(ProtocolVersion::LATEST))
                .await
                .unwrap();
            // list capability must be present
            assert!(
                resp.agent_capabilities
                    .session_capabilities
                    .list
                    .is_some(),
                "list session capability must be set"
            );
        })
        .await;
}

// ── new_session ───────────────────────────────────────────────────────────────

#[tokio::test]
async fn new_session_saves_state_and_returns_id() {
    let (store, _, agent) = make_agent_parts();

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let resp = agent
                .new_session(NewSessionRequest::new("/home/user/project"))
                .await
                .unwrap();
            let session_id = resp.session_id.to_string();
            assert!(!session_id.is_empty());

            let state = store.load(&session_id).await.unwrap();
            assert_eq!(state.cwd, "/home/user/project");
            assert!(!state.created_at.is_empty());
        })
        .await;
}

#[tokio::test]
async fn new_session_default_mode_is_default() {
    let (store, _, agent) = make_agent_parts();

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let resp = agent
                .new_session(NewSessionRequest::new("/cwd"))
                .await
                .unwrap();
            let state = store.load(&resp.session_id.to_string()).await.unwrap();
            assert_eq!(state.mode, "default");
        })
        .await;
}

#[tokio::test]
async fn new_session_publishes_session_ready() {
    let (_, notifier, agent) = make_agent_parts();

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            agent
                .new_session(NewSessionRequest::new("/cwd"))
                .await
                .unwrap();
            let published = notifier.published();
            assert_eq!(published.len(), 1, "one session.ready must be published");
        })
        .await;
}

// ── load_session ──────────────────────────────────────────────────────────────

#[tokio::test]
async fn load_session_returns_ok_for_missing_session() {
    let agent = make_agent();
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let resp = agent
                .load_session(LoadSessionRequest::new("nonexistent-id", "/cwd"))
                .await;
            assert!(resp.is_ok());
        })
        .await;
}

// ── set_session_mode ──────────────────────────────────────────────────────────

#[tokio::test]
async fn set_session_mode_updates_stored_mode() {
    let (store, _, agent) = make_agent_parts();

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let resp = agent
                .new_session(NewSessionRequest::new("/cwd"))
                .await
                .unwrap();
            let session_id = resp.session_id.to_string();

            agent
                .set_session_mode(SetSessionModeRequest::new(session_id.clone(), "plan"))
                .await
                .unwrap();

            let state = store.load(&session_id).await.unwrap();
            assert_eq!(state.mode, "plan");
        })
        .await;
}

// ── set_session_model ─────────────────────────────────────────────────────────

#[tokio::test]
async fn set_session_model_updates_stored_model() {
    let (store, _, agent) = make_agent_parts();

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let resp = agent
                .new_session(NewSessionRequest::new("/cwd"))
                .await
                .unwrap();
            let session_id = resp.session_id.to_string();

            agent
                .set_session_model(SetSessionModelRequest::new(
                    session_id.clone(),
                    "claude-sonnet-4-6",
                ))
                .await
                .unwrap();

            let state = store.load(&session_id).await.unwrap();
            assert_eq!(state.model.as_deref(), Some("claude-sonnet-4-6"));
        })
        .await;
}

// ── list_sessions ─────────────────────────────────────────────────────────────

#[tokio::test]
async fn list_sessions_returns_all_sessions() {
    let (_, _, agent) = make_agent_parts();

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            agent
                .new_session(NewSessionRequest::new("/a"))
                .await
                .unwrap();
            agent
                .new_session(NewSessionRequest::new("/b"))
                .await
                .unwrap();

            let resp = agent
                .list_sessions(ListSessionsRequest::new())
                .await
                .unwrap();
            assert_eq!(resp.sessions.len(), 2);
        })
        .await;
}

#[tokio::test]
async fn list_sessions_branch_has_parent_meta() {
    let (_, _, agent) = make_agent_parts();

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let parent_resp = agent
                .new_session(NewSessionRequest::new("/root"))
                .await
                .unwrap();
            let parent_id = parent_resp.session_id.to_string();

            let fork_resp = agent
                .fork_session(ForkSessionRequest::new(parent_id.clone(), "/branch"))
                .await
                .unwrap();
            let fork_id = fork_resp.session_id.to_string();

            let list_resp = agent
                .list_sessions(ListSessionsRequest::new())
                .await
                .unwrap();

            let branch_info = list_resp
                .sessions
                .iter()
                .find(|s| s.session_id.to_string() == fork_id)
                .expect("forked session must appear in list");

            let meta = branch_info.meta.as_ref().expect("branch must have _meta");
            assert_eq!(
                meta.get("parentSessionId").and_then(|v| v.as_str()),
                Some(parent_id.as_str()),
                "parentSessionId must be present in _meta"
            );

            let root_info = list_resp
                .sessions
                .iter()
                .find(|s| s.session_id.to_string() == parent_id)
                .expect("parent session must appear in list");
            assert!(
                root_info.meta.is_none(),
                "root session must not have branch _meta"
            );
        })
        .await;
}

// ── fork_session ──────────────────────────────────────────────────────────────

#[tokio::test]
async fn fork_session_creates_copy_with_new_id() {
    let (store, _, agent) = make_agent_parts();

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let new_resp = agent
                .new_session(NewSessionRequest::new("/src"))
                .await
                .unwrap();
            let source_id = new_resp.session_id.to_string();

            let fork_resp = agent
                .fork_session(ForkSessionRequest::new(source_id.clone(), "/src"))
                .await
                .unwrap();
            let fork_id = fork_resp.session_id.to_string();

            assert_ne!(source_id, fork_id, "forked session must have a different ID");

            let fork_state = store.load(&fork_id).await.unwrap();
            assert_eq!(fork_state.cwd, "/src");
        })
        .await;
}

#[tokio::test]
async fn fork_session_records_parent_id() {
    let (store, _, agent) = make_agent_parts();

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let new_resp = agent
                .new_session(NewSessionRequest::new("/src"))
                .await
                .unwrap();
            let source_id = new_resp.session_id.to_string();

            let fork_resp = agent
                .fork_session(ForkSessionRequest::new(source_id.clone(), "/fork"))
                .await
                .unwrap();
            let fork_id = fork_resp.session_id.to_string();

            let fork_state = store.load(&fork_id).await.unwrap();
            assert_eq!(
                fork_state.parent_session_id.as_deref(),
                Some(source_id.as_str()),
                "fork must record parent session ID"
            );
            assert_eq!(fork_state.branched_at_index, None);
        })
        .await;
}

#[tokio::test]
async fn fork_session_branches_at_index() {
    let (store, _, agent) = make_agent_parts();

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let new_resp = agent
                .new_session(NewSessionRequest::new("/src"))
                .await
                .unwrap();
            let source_id = new_resp.session_id.to_string();

            let mut state = store.load(&source_id).await.unwrap();
            state.messages = (0..4)
                .map(|i| Message::user_text(format!("msg-{i}")))
                .collect();
            store.save(&source_id, &state).await.unwrap();

            let meta = serde_json::from_value::<serde_json::Map<String, serde_json::Value>>(
                serde_json::json!({ "branchAtIndex": 2 }),
            )
            .unwrap();
            let fork_resp = agent
                .fork_session(ForkSessionRequest::new(source_id.clone(), "/branch").meta(meta))
                .await
                .unwrap();
            let fork_id = fork_resp.session_id.to_string();

            let fork_state = store.load(&fork_id).await.unwrap();
            assert_eq!(fork_state.messages.len(), 2, "branch must truncate to index 2");
            assert_eq!(fork_state.branched_at_index, Some(2));
            assert_eq!(
                fork_state.parent_session_id.as_deref(),
                Some(source_id.as_str())
            );

            let src_state = store.load(&source_id).await.unwrap();
            assert_eq!(src_state.messages.len(), 4, "source must remain unchanged");
        })
        .await;
}

#[tokio::test]
async fn fork_session_branch_at_index_out_of_bounds_copies_full_history() {
    let (store, _, agent) = make_agent_parts();

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let new_resp = agent
                .new_session(NewSessionRequest::new("/src"))
                .await
                .unwrap();
            let source_id = new_resp.session_id.to_string();

            let mut state = store.load(&source_id).await.unwrap();
            state.messages = (0..3)
                .map(|i| Message::user_text(format!("msg-{i}")))
                .collect();
            store.save(&source_id, &state).await.unwrap();

            let meta = serde_json::from_value::<serde_json::Map<String, serde_json::Value>>(
                serde_json::json!({ "branchAtIndex": 99 }),
            )
            .unwrap();
            let fork_resp = agent
                .fork_session(ForkSessionRequest::new(source_id.clone(), "/branch").meta(meta))
                .await
                .unwrap();
            let fork_id = fork_resp.session_id.to_string();

            let fork_state = store.load(&fork_id).await.unwrap();
            assert_eq!(
                fork_state.messages.len(),
                3,
                "out-of-bounds branchAtIndex must copy the full history"
            );
            assert_eq!(fork_state.branched_at_index, Some(99));
        })
        .await;
}

#[tokio::test]
async fn fork_session_branch_at_index_zero_produces_empty_history() {
    let (store, _, agent) = make_agent_parts();

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let new_resp = agent
                .new_session(NewSessionRequest::new("/src"))
                .await
                .unwrap();
            let source_id = new_resp.session_id.to_string();

            let mut state = store.load(&source_id).await.unwrap();
            state.messages = (0..3)
                .map(|i| Message::user_text(format!("msg-{i}")))
                .collect();
            store.save(&source_id, &state).await.unwrap();

            let meta = serde_json::from_value::<serde_json::Map<String, serde_json::Value>>(
                serde_json::json!({ "branchAtIndex": 0 }),
            )
            .unwrap();
            let fork_resp = agent
                .fork_session(ForkSessionRequest::new(source_id.clone(), "/branch").meta(meta))
                .await
                .unwrap();
            let fork_id = fork_resp.session_id.to_string();

            let fork_state = store.load(&fork_id).await.unwrap();
            assert_eq!(
                fork_state.messages.len(),
                0,
                "branchAtIndex: 0 must produce an empty history"
            );
            assert_eq!(fork_state.branched_at_index, Some(0));

            let src_state = store.load(&source_id).await.unwrap();
            assert_eq!(src_state.messages.len(), 3, "source must remain unchanged");
        })
        .await;
}

// ── close_session ─────────────────────────────────────────────────────────────

#[tokio::test]
async fn close_session_deletes_stored_state() {
    let (store, _, agent) = make_agent_parts();

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let resp = agent
                .new_session(NewSessionRequest::new("/tmp"))
                .await
                .unwrap();
            let session_id = resp.session_id.to_string();

            let ids_before = store.list_ids().await.unwrap();
            assert!(ids_before.contains(&session_id));

            agent
                .close_session(CloseSessionRequest::new(session_id.clone()))
                .await
                .unwrap();

            let ids_after = store.list_ids().await.unwrap();
            assert!(
                !ids_after.contains(&session_id),
                "session must be removed after close"
            );
        })
        .await;
}

// ── resume_session ────────────────────────────────────────────────────────────

#[tokio::test]
async fn resume_session_returns_ok() {
    let agent = make_agent();
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let result = agent
                .resume_session(ResumeSessionRequest::new("any-session", "/cwd"))
                .await;
            assert!(result.is_ok());
        })
        .await;
}

// ── ext_method: session/list_children ────────────────────────────────────────

#[tokio::test]
async fn ext_list_children_returns_direct_children() {
    let (store, _, agent) = make_agent_parts();

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let parent_resp = agent
                .new_session(NewSessionRequest::new("/parent"))
                .await
                .unwrap();
            let parent_id = parent_resp.session_id.to_string();

            let fork1 = agent
                .fork_session(ForkSessionRequest::new(parent_id.clone(), "/branch1"))
                .await
                .unwrap();
            let fork2 = agent
                .fork_session(ForkSessionRequest::new(parent_id.clone(), "/branch2"))
                .await
                .unwrap();

            let params_json = format!(r#"{{"sessionId":"{}"}}"#, parent_id);
            let params: std::sync::Arc<serde_json::value::RawValue> =
                serde_json::value::RawValue::from_string(params_json)
                    .unwrap()
                    .into();
            let resp = agent
                .ext_method(ExtRequest::new("session/list_children", params))
                .await
                .unwrap();

            let body: serde_json::Value =
                serde_json::from_str(resp.0.get()).unwrap();
            let mut children: Vec<String> = body["children"]
                .as_array()
                .unwrap()
                .iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect();
            children.sort();

            let mut expected =
                vec![fork1.session_id.to_string(), fork2.session_id.to_string()];
            expected.sort();
            assert_eq!(children, expected);

            let _ = store.list_children(&parent_id).await.unwrap();
        })
        .await;
}

#[tokio::test]
async fn ext_list_children_returns_empty_for_root_session() {
    let (_, _, agent) = make_agent_parts();

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let resp = agent
                .new_session(NewSessionRequest::new("/root"))
                .await
                .unwrap();
            let sid = resp.session_id.to_string();

            let params_json = format!(r#"{{"sessionId":"{}"}}"#, sid);
            let params: std::sync::Arc<serde_json::value::RawValue> =
                serde_json::value::RawValue::from_string(params_json)
                    .unwrap()
                    .into();
            let resp = agent
                .ext_method(ExtRequest::new("session/list_children", params))
                .await
                .unwrap();

            let body: serde_json::Value =
                serde_json::from_str(resp.0.get()).unwrap();
            let children = body["children"].as_array().unwrap();
            assert!(children.is_empty(), "root session must have no children");
        })
        .await;
}

#[tokio::test]
async fn ext_list_children_only_returns_direct_children_not_grandchildren() {
    let (_, _, agent) = make_agent_parts();

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            // A → B → C
            let a = agent
                .new_session(NewSessionRequest::new("/a"))
                .await
                .unwrap()
                .session_id
                .to_string();
            let b = agent
                .fork_session(ForkSessionRequest::new(a.clone(), "/b"))
                .await
                .unwrap()
                .session_id
                .to_string();
            let c = agent
                .fork_session(ForkSessionRequest::new(b.clone(), "/c"))
                .await
                .unwrap()
                .session_id
                .to_string();

            let call = |sid: String| {
                let params_json = format!(r#"{{"sessionId":"{}"}}"#, sid);
                let params: std::sync::Arc<serde_json::value::RawValue> =
                    serde_json::value::RawValue::from_string(params_json)
                        .unwrap()
                        .into();
                ExtRequest::new("session/list_children", params)
            };

            let children_a: Vec<String> = {
                let body: serde_json::Value = serde_json::from_str(
                    agent.ext_method(call(a.clone())).await.unwrap().0.get(),
                )
                .unwrap();
                body["children"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|v| v.as_str().unwrap().to_string())
                    .collect()
            };
            assert_eq!(children_a, vec![b.clone()], "A must have only B as child");

            let children_b: Vec<String> = {
                let body: serde_json::Value = serde_json::from_str(
                    agent.ext_method(call(b.clone())).await.unwrap().0.get(),
                )
                .unwrap();
                body["children"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|v| v.as_str().unwrap().to_string())
                    .collect()
            };
            assert_eq!(children_b, vec![c.clone()], "B must have only C as child");

            let children_c: Vec<String> = {
                let body: serde_json::Value = serde_json::from_str(
                    agent.ext_method(call(c.clone())).await.unwrap().0.get(),
                )
                .unwrap();
                body["children"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|v| v.as_str().unwrap().to_string())
                    .collect()
            };
            assert!(children_c.is_empty(), "C must have no children");
        })
        .await;
}

#[tokio::test]
async fn ext_unknown_method_returns_method_not_found() {
    let (_, _, agent) = make_agent_parts();

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let params: std::sync::Arc<serde_json::value::RawValue> =
                serde_json::value::RawValue::from_string("{}".to_string())
                    .unwrap()
                    .into();
            let err = agent
                .ext_method(ExtRequest::new("session/nope", params))
                .await
                .unwrap_err();
            assert!(err.to_string().contains("unknown ext method"));
        })
        .await;
}

// ── fork_session: notifications and state inheritance ────────────────────────

#[tokio::test]
async fn fork_session_publishes_session_ready() {
    let (_, notifier, agent) = make_agent_parts();

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let parent_resp = agent
                .new_session(NewSessionRequest::new("/src"))
                .await
                .unwrap();
            let parent_id = parent_resp.session_id.to_string();

            // One notification for new_session; fork must add a second one.
            agent
                .fork_session(ForkSessionRequest::new(parent_id, "/fork"))
                .await
                .unwrap();

            let published = notifier.published();
            assert_eq!(published.len(), 2, "fork_session must publish a session.ready notification");
        })
        .await;
}

#[tokio::test]
async fn fork_session_inherits_parent_mode_and_model() {
    let (store, _, agent) = make_agent_parts();

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let parent_resp = agent
                .new_session(NewSessionRequest::new("/src"))
                .await
                .unwrap();
            let parent_id = parent_resp.session_id.to_string();

            // Mutate mode and model directly in the store before forking.
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
            assert_eq!(fork_state.mode, "plan", "fork must inherit parent mode");
            assert_eq!(
                fork_state.model.as_deref(),
                Some("claude-opus-4-7"),
                "fork must inherit parent model"
            );
        })
        .await;
}

// ── fork_session: edge cases ──────────────────────────────────────────────────

#[tokio::test]
async fn fork_session_nonexistent_source_succeeds_with_default_state() {
    // MemorySessionStore::load returns Ok(Default) for unknown IDs, so forking
    // a session that was never created silently succeeds.  This documents the
    // current semantics: no "session not found" error is raised.
    let (store, _, agent) = make_agent_parts();

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let fork_resp = agent
                .fork_session(ForkSessionRequest::new("ghost-session-id", "/fork"))
                .await;

            assert!(fork_resp.is_ok(), "fork of non-existent session must succeed");
            let fork_id = fork_resp.unwrap().session_id.to_string();
            assert!(!fork_id.is_empty());

            let fork_state = store.load(&fork_id).await.unwrap();
            assert_eq!(
                fork_state.parent_session_id.as_deref(),
                Some("ghost-session-id"),
                "fork must record the (non-existent) parent session ID"
            );
            assert!(fork_state.messages.is_empty(), "forked state must be empty (default)");
        })
        .await;
}

#[tokio::test]
async fn fork_session_branch_at_index_wrong_type_falls_back_to_full_copy() {
    // branchAtIndex must be a JSON integer.  A JSON string (e.g. "2") is not
    // parsed by as_u64(), so branch_at stays None → full copy, branched_at_index = None.
    let (store, _, agent) = make_agent_parts();

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let new_resp = agent
                .new_session(NewSessionRequest::new("/src"))
                .await
                .unwrap();
            let source_id = new_resp.session_id.to_string();

            let mut state = store.load(&source_id).await.unwrap();
            state.messages = (0..4)
                .map(|i| Message::user_text(format!("msg-{i}")))
                .collect();
            store.save(&source_id, &state).await.unwrap();

            // Pass branchAtIndex as a JSON *string* — must be silently ignored.
            let meta = serde_json::from_value::<serde_json::Map<String, serde_json::Value>>(
                serde_json::json!({ "branchAtIndex": "2" }),
            )
            .unwrap();
            let fork_resp = agent
                .fork_session(ForkSessionRequest::new(source_id.clone(), "/branch").meta(meta))
                .await
                .unwrap();
            let fork_id = fork_resp.session_id.to_string();

            let fork_state = store.load(&fork_id).await.unwrap();
            assert_eq!(
                fork_state.messages.len(),
                4,
                "wrong-type branchAtIndex must be ignored → full history copied"
            );
            assert_eq!(
                fork_state.branched_at_index,
                None,
                "branched_at_index must be None when branchAtIndex had a wrong type"
            );
        })
        .await;
}

// ── ext_method: session/list_children edge cases ──────────────────────────────

#[tokio::test]
async fn ext_list_children_missing_session_id_param_returns_empty() {
    // When params contains no sessionId field, unwrap_or_default() → "" → no
    // session has "" as parent → empty children list.
    let (_, _, agent) = make_agent_parts();

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let params: std::sync::Arc<serde_json::value::RawValue> =
                serde_json::value::RawValue::from_string("{}".to_string())
                    .unwrap()
                    .into();
            let resp = agent
                .ext_method(ExtRequest::new("session/list_children", params))
                .await
                .unwrap();
            let body: serde_json::Value = serde_json::from_str(resp.0.get()).unwrap();
            let children = body["children"].as_array().unwrap();
            assert!(
                children.is_empty(),
                "missing sessionId must yield an empty children list"
            );
        })
        .await;
}

#[tokio::test]
async fn ext_list_children_null_session_id_returns_empty() {
    // sessionId: null is treated the same as missing — as_str() returns None,
    // unwrap_or_default() → "" → no match → empty.
    let (_, _, agent) = make_agent_parts();

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let params: std::sync::Arc<serde_json::value::RawValue> =
                serde_json::value::RawValue::from_string(r#"{"sessionId":null}"#.to_string())
                    .unwrap()
                    .into();
            let resp = agent
                .ext_method(ExtRequest::new("session/list_children", params))
                .await
                .unwrap();
            let body: serde_json::Value = serde_json::from_str(resp.0.get()).unwrap();
            let children = body["children"].as_array().unwrap();
            assert!(
                children.is_empty(),
                "null sessionId must yield an empty children list"
            );
        })
        .await;
}

// ── list_sessions: _meta fields ───────────────────────────────────────────────

#[tokio::test]
async fn list_sessions_includes_branched_at_index_in_meta() {
    let (_, _, agent) = make_agent_parts();

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let parent_resp = agent
                .new_session(NewSessionRequest::new("/root"))
                .await
                .unwrap();
            let parent_id = parent_resp.session_id.to_string();

            let meta = serde_json::from_value::<serde_json::Map<String, serde_json::Value>>(
                serde_json::json!({ "branchAtIndex": 3 }),
            )
            .unwrap();
            let fork_resp = agent
                .fork_session(ForkSessionRequest::new(parent_id.clone(), "/fork").meta(meta))
                .await
                .unwrap();
            let fork_id = fork_resp.session_id.to_string();

            let list_resp = agent
                .list_sessions(ListSessionsRequest::new())
                .await
                .unwrap();

            let fork_info = list_resp
                .sessions
                .iter()
                .find(|s| s.session_id.to_string() == fork_id)
                .expect("forked session must appear in list_sessions");

            let m = fork_info.meta.as_ref().expect("fork must have _meta");
            assert_eq!(
                m.get("branchedAtIndex").and_then(|v| v.as_u64()),
                Some(3),
                "list_sessions _meta must include branchedAtIndex"
            );
            assert_eq!(
                m.get("parentSessionId").and_then(|v| v.as_str()),
                Some(parent_id.as_str()),
                "list_sessions _meta must include parentSessionId"
            );
        })
        .await;
}

#[tokio::test]
async fn list_sessions_preserves_parent_session_id_after_parent_deleted() {
    // When the parent session is deleted, the fork's _meta.parentSessionId must
    // still appear — the dangling reference is not cleaned up.
    let (store, _, agent) = make_agent_parts();

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let parent_resp = agent
                .new_session(NewSessionRequest::new("/root"))
                .await
                .unwrap();
            let parent_id = parent_resp.session_id.to_string();

            let fork_resp = agent
                .fork_session(ForkSessionRequest::new(parent_id.clone(), "/fork"))
                .await
                .unwrap();
            let fork_id = fork_resp.session_id.to_string();

            // Delete the parent directly via the store.
            store.delete(&parent_id).await.unwrap();

            let list_resp = agent
                .list_sessions(ListSessionsRequest::new())
                .await
                .unwrap();

            // Parent must no longer appear.
            assert!(
                list_resp
                    .sessions
                    .iter()
                    .all(|s| s.session_id.to_string() != parent_id),
                "deleted parent must not appear in list_sessions"
            );

            // Fork must still expose the dangling parentSessionId in _meta.
            let fork_info = list_resp
                .sessions
                .iter()
                .find(|s| s.session_id.to_string() == fork_id)
                .expect("fork session must still appear");
            let m = fork_info.meta.as_ref().expect("fork must have _meta");
            assert_eq!(
                m.get("parentSessionId").and_then(|v| v.as_str()),
                Some(parent_id.as_str()),
                "parentSessionId must survive even after the parent is deleted"
            );
        })
        .await;
}

// ── cancel ────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn cancel_publishes_cancel_subject() {
    let (_, notifier, agent) = make_agent_parts();

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            agent
                .cancel(CancelNotification::new("sess-xyz"))
                .await
                .unwrap();

            let published = notifier.published();
            assert_eq!(published.len(), 1);
            assert!(
                published[0].0.contains("sess-xyz"),
                "cancel subject must contain session ID: {:?}",
                published[0].0
            );
        })
        .await;
}

// ── prompt ────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn prompt_runs_agent_and_saves_messages() {
    let store = MemorySessionStore::new();
    let reply = vec![
        Message::user_text("hello"),
        Message {
            role: "assistant".to_string(),
            content: vec![AgentContentBlock::Text {
                text: "Hi there!".to_string(),
            }],
        },
    ];
    let runner = MockAgentRunner::new("claude-test").with_response(reply);
    let agent = TrogonAgent::new(
        MockSessionNotifier::new(),
        store.clone(),
        runner,
        "acp",
        "claude-test",
        None,
        None,
        Arc::new(RwLock::new(None::<GatewayConfig>)),
    );

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let new_resp = agent
                .new_session(NewSessionRequest::new("/cwd"))
                .await
                .unwrap();
            let session_id = new_resp.session_id.to_string();

            let prompt_req = PromptRequest::new(
                session_id.clone(),
                vec![agent_client_protocol::ContentBlock::Text(
                    TextContent::new("hello"),
                )],
            );
            let result = agent.prompt(prompt_req).await;
            assert!(result.is_ok(), "prompt must succeed: {:?}", result);

            let state = store.load(&session_id).await.unwrap();
            assert!(
                !state.messages.is_empty(),
                "session must contain messages after prompt"
            );
        })
        .await;
}

#[tokio::test]
async fn prompt_sets_title_from_first_text_block() {
    let (store, _, agent) = make_agent_parts();

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let new_resp = agent
                .new_session(NewSessionRequest::new("/cwd"))
                .await
                .unwrap();
            let session_id = new_resp.session_id.to_string();

            let prompt_req = PromptRequest::new(
                session_id.clone(),
                vec![agent_client_protocol::ContentBlock::Text(
                    TextContent::new("What is Rust?"),
                )],
            );
            agent.prompt(prompt_req).await.unwrap();

            let state = store.load(&session_id).await.unwrap();
            assert_eq!(state.title, "What is Rust?");
        })
        .await;
}

#[tokio::test]
async fn prompt_max_iterations_returns_max_turn_requests() {
    use agent_client_protocol::StopReason;

    let store = MemorySessionStore::new();
    let runner = MockAgentRunner::new("claude-test").with_error(AgentError::MaxIterationsReached);
    let agent = TrogonAgent::new(
        MockSessionNotifier::new(),
        store,
        runner,
        "acp",
        "claude-test",
        None,
        None,
        Arc::new(RwLock::new(None::<GatewayConfig>)),
    );

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let new_resp = agent
                .new_session(NewSessionRequest::new("/cwd"))
                .await
                .unwrap();
            let session_id = new_resp.session_id.to_string();

            let prompt_req = PromptRequest::new(
                session_id,
                vec![agent_client_protocol::ContentBlock::Text(
                    TextContent::new("loop"),
                )],
            );
            let result = agent.prompt(prompt_req).await.unwrap();
            assert_eq!(result.stop_reason, StopReason::MaxTurnRequests);
        })
        .await;
}

#[tokio::test]
async fn prompt_max_tokens_returns_max_tokens_stop_reason() {
    use agent_client_protocol::StopReason;

    let store = MemorySessionStore::new();
    let runner = MockAgentRunner::new("claude-test").with_error(AgentError::MaxTokens);
    let agent = TrogonAgent::new(
        MockSessionNotifier::new(),
        store,
        runner,
        "acp",
        "claude-test",
        None,
        None,
        Arc::new(RwLock::new(None::<GatewayConfig>)),
    );

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let new_resp = agent
                .new_session(NewSessionRequest::new("/cwd"))
                .await
                .unwrap();
            let session_id = new_resp.session_id.to_string();

            let prompt_req = PromptRequest::new(
                session_id,
                vec![agent_client_protocol::ContentBlock::Text(
                    TextContent::new("big prompt"),
                )],
            );
            let result = agent.prompt(prompt_req).await.unwrap();
            assert_eq!(result.stop_reason, StopReason::MaxTokens);
        })
        .await;
}

// ── steer ─────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn prompt_steer_subject_contains_session_id() {
    let store = MemorySessionStore::new();
    let notifier = MockSessionNotifier::new();
    let agent = TrogonAgent::new(
        notifier.clone(),
        store.clone(),
        MockAgentRunner::new("claude-test"),
        "acp",
        "claude-test",
        None,
        None,
        Arc::new(RwLock::new(None::<GatewayConfig>)),
    );

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let new_resp = agent
                .new_session(NewSessionRequest::new("/cwd"))
                .await
                .unwrap();
            let session_id = new_resp.session_id.to_string();

            let prompt_req = PromptRequest::new(session_id.clone(), vec![]);
            agent.prompt(prompt_req).await.unwrap();

            let subjects = notifier.steer_subjects();
            assert_eq!(subjects.len(), 1, "subscribe_steer must be called once per prompt");
            assert!(
                subjects[0].contains(&session_id),
                "steer subject must contain session id; got: {}",
                subjects[0]
            );
            assert!(
                subjects[0].contains("steer"),
                "steer subject must contain 'steer'; got: {}",
                subjects[0]
            );
        })
        .await;
}

#[tokio::test]
async fn prompt_steer_message_reaches_runner() {
    let store = MemorySessionStore::new();
    let notifier = MockSessionNotifier::new();
    notifier.inject_steer_message("think carefully about safety");
    let runner = MockAgentRunner::new("claude-test");
    let agent = TrogonAgent::new(
        notifier,
        store.clone(),
        runner.clone(),
        "acp",
        "claude-test",
        None,
        None,
        Arc::new(RwLock::new(None::<GatewayConfig>)),
    );

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let new_resp = agent
                .new_session(NewSessionRequest::new("/cwd"))
                .await
                .unwrap();
            let session_id = new_resp.session_id.to_string();

            let prompt_req = PromptRequest::new(session_id, vec![]);
            agent.prompt(prompt_req).await.unwrap();

            let steer = runner.captured_steer();
            assert_eq!(steer.len(), 1, "runner must receive exactly one steer message");
            assert_eq!(steer[0], "think carefully about safety");
        })
        .await;
}

#[tokio::test]
async fn prompt_multiple_steer_messages_all_reach_runner() {
    let store = MemorySessionStore::new();
    let notifier = MockSessionNotifier::new();
    notifier.inject_steer_message("first guidance");
    notifier.inject_steer_message("second guidance");
    notifier.inject_steer_message("third guidance");
    let runner = MockAgentRunner::new("claude-test");
    let agent = TrogonAgent::new(
        notifier,
        store.clone(),
        runner.clone(),
        "acp",
        "claude-test",
        None,
        None,
        Arc::new(RwLock::new(None::<GatewayConfig>)),
    );

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let new_resp = agent
                .new_session(NewSessionRequest::new("/cwd"))
                .await
                .unwrap();
            let session_id = new_resp.session_id.to_string();

            let prompt_req = PromptRequest::new(session_id, vec![]);
            agent.prompt(prompt_req).await.unwrap();

            let steer = runner.captured_steer();
            assert_eq!(steer.len(), 3, "all three steer messages must reach the runner");
            assert_eq!(steer[0], "first guidance");
            assert_eq!(steer[1], "second guidance");
            assert_eq!(steer[2], "third guidance");
        })
        .await;
}

#[tokio::test]
async fn prompt_no_steer_runner_receives_empty() {
    let store = MemorySessionStore::new();
    let notifier = MockSessionNotifier::new();
    let runner = MockAgentRunner::new("claude-test");
    let agent = TrogonAgent::new(
        notifier,
        store.clone(),
        runner.clone(),
        "acp",
        "claude-test",
        None,
        None,
        Arc::new(RwLock::new(None::<GatewayConfig>)),
    );

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let new_resp = agent
                .new_session(NewSessionRequest::new("/cwd"))
                .await
                .unwrap();
            let session_id = new_resp.session_id.to_string();

            let prompt_req = PromptRequest::new(session_id, vec![]);
            agent.prompt(prompt_req).await.unwrap();

            assert!(
                runner.captured_steer().is_empty(),
                "runner must not receive steer messages when none were injected"
            );
        })
        .await;
}

#[tokio::test]
async fn prompt_steer_subscribe_failure_prompt_still_succeeds() {
    // When subscribe_steer returns None (e.g. NATS subscribe failed),
    // TrogonAgent must continue the prompt with no steer (steer_rx = None)
    // rather than returning an error.
    let store = MemorySessionStore::new();
    let notifier = MockSessionNotifier::new();
    notifier.fail_steer_subscribe(); // next subscribe_steer → None
    let runner = MockAgentRunner::new("claude-test");
    let agent = TrogonAgent::new(
        notifier,
        store.clone(),
        runner.clone(),
        "acp",
        "claude-test",
        None,
        None,
        Arc::new(RwLock::new(None::<GatewayConfig>)),
    );

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let new_resp = agent
                .new_session(NewSessionRequest::new("/cwd"))
                .await
                .unwrap();
            let session_id = new_resp.session_id.to_string();

            let result = agent.prompt(PromptRequest::new(session_id, vec![])).await;
            assert!(
                result.is_ok(),
                "prompt must succeed even when subscribe_steer fails: {result:?}"
            );
            assert!(
                runner.captured_steer().is_empty(),
                "no steer messages must reach runner when subscription failed"
            );
        })
        .await;
}

#[tokio::test]
async fn prompt_text_delta_events_are_forwarded_without_error() {
    let runner = MockAgentRunner::new("claude-test").with_events(vec![
        AgentEvent::TextDelta {
            text: "Hello".to_string(),
        },
        AgentEvent::TextDelta {
            text: " world".to_string(),
        },
    ]);
    let agent = TrogonAgent::new(
        MockSessionNotifier::new(),
        MemorySessionStore::new(),
        runner,
        "acp",
        "claude-test",
        None,
        None,
        Arc::new(RwLock::new(None::<GatewayConfig>)),
    );

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let new_resp = agent
                .new_session(NewSessionRequest::new("/cwd"))
                .await
                .unwrap();
            let session_id = new_resp.session_id.to_string();
            let prompt_req = PromptRequest::new(session_id, vec![]);
            let result = agent.prompt(prompt_req).await;
            assert!(result.is_ok(), "prompt with events must succeed");
        })
        .await;
}

// ── execution backend ─────────────────────────────────────────────────────────

#[tokio::test]
async fn no_execution_backend_bash_tool_not_injected() {
    // When with_execution_backend is not called, the registry is None and
    // add_mcp_tools must never be called with a "bash" tool def.
    let runner = MockAgentRunner::new("claude-test");
    let agent = TrogonAgent::new(
        MockSessionNotifier::new(),
        MemorySessionStore::new(),
        runner.clone(),
        "acp",
        "claude-test",
        None,
        None,
        Arc::new(RwLock::new(None::<GatewayConfig>)),
    );

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let resp = agent
                .new_session(NewSessionRequest::new("/cwd"))
                .await
                .unwrap();
            let session_id = resp.session_id.to_string();
            agent
                .prompt(PromptRequest::new(session_id, vec![]))
                .await
                .unwrap();
        })
        .await;

    assert!(
        !runner.captured_tool_names().contains(&"bash".to_string()),
        "bash tool must not be injected when no execution backend is configured, got: {:?}",
        runner.captured_tool_names()
    );
}

/// When `TrogonAgent` is constructed with a non-None `elicitation_tx`, the
/// `prompt()` path must call `set_elicitation_provider` on the cloned runner
/// before dispatching the request.
#[tokio::test]
async fn prompt_injects_elicitation_provider_when_elicitation_tx_is_some() {
    let (elic_tx, _elic_rx): (ElicitationTx, _) = mpsc::channel(8);

    let runner = MockAgentRunner::new("claude-test");
    let agent = TrogonAgent::new(
        MockSessionNotifier::new(),
        MemorySessionStore::new(),
        runner.clone(),
        "acp",
        "claude-test",
        None,
        Some(elic_tx),
        Arc::new(RwLock::new(None::<GatewayConfig>)),
    );

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let resp = agent
                .new_session(NewSessionRequest::new("/cwd"))
                .await
                .unwrap();
            let session_id = resp.session_id.to_string();
            agent
                .prompt(PromptRequest::new(session_id, vec![]))
                .await
                .unwrap();
        })
        .await;

    assert!(
        *runner.elicitation_provider_set.lock().unwrap(),
        "set_elicitation_provider must be called when elicitation_tx is Some"
    );
}

/// When `TrogonAgent` is constructed with `elicitation_tx: None`, `prompt()`
/// must NOT call `set_elicitation_provider`.
#[tokio::test]
async fn prompt_does_not_inject_elicitation_provider_when_elicitation_tx_is_none() {
    let runner = MockAgentRunner::new("claude-test");
    let agent = TrogonAgent::new(
        MockSessionNotifier::new(),
        MemorySessionStore::new(),
        runner.clone(),
        "acp",
        "claude-test",
        None,
        None,
        Arc::new(RwLock::new(None::<GatewayConfig>)),
    );

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let resp = agent
                .new_session(NewSessionRequest::new("/cwd"))
                .await
                .unwrap();
            let session_id = resp.session_id.to_string();
            agent
                .prompt(PromptRequest::new(session_id, vec![]))
                .await
                .unwrap();
        })
        .await;

    assert!(
        !*runner.elicitation_provider_set.lock().unwrap(),
        "set_elicitation_provider must NOT be called when elicitation_tx is None"
    );
}

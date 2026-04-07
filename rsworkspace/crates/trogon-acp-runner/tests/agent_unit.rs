//! Unit tests for `TrogonAgent` using in-memory mocks.
//!
//! No Docker, no NATS — all dependencies are replaced by in-process stubs.
//!
//! Run with:
//!   cargo test -p trogon-acp-runner --test agent_unit --features test-helpers

use std::sync::Arc;

use agent_client_protocol::{
    Agent, CancelNotification, CloseSessionRequest, ForkSessionRequest, InitializeRequest,
    ListSessionsRequest, LoadSessionRequest, NewSessionRequest, PromptRequest, ProtocolVersion,
    ResumeSessionRequest, SetSessionModeRequest, SetSessionModelRequest, TextContent,
};
use tokio::sync::RwLock;
use trogon_acp_runner::{
    GatewayConfig, SessionStore, TrogonAgent,
    agent_runner::mock::MockAgentRunner,
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

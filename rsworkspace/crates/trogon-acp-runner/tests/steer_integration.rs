//! NATS integration tests for the steer feature.
//!
//! Verifies that a NATS publish to `{prefix}.session.{id}.agent.steer` flows
//! through `NatsSessionNotifier::subscribe_steer` into the runner's `steer_rx`.
//!
//! Requires Docker (uses testcontainers to spin up a NATS server).
//!
//! Run with:
//!   cargo test -p trogon-acp-runner --test steer_integration --features test-helpers

use std::sync::Arc;
use std::time::Duration;

use agent_client_protocol::{Agent, NewSessionRequest, PromptRequest};
use testcontainers_modules::nats::Nats;
use testcontainers_modules::testcontainers::runners::AsyncRunner;
use tokio::sync::RwLock;
use trogon_acp_runner::{
    GatewayConfig, TrogonAgent,
    agent_runner::mock::MockAgentRunner,
    session_notifier::NatsSessionNotifier,
    session_store::mock::MemorySessionStore,
};

async fn start_nats() -> (testcontainers_modules::testcontainers::ContainerAsync<Nats>, u16) {
    let container = Nats::default()
        .start()
        .await
        .expect("Failed to start NATS container — is Docker running?");
    let port = container.get_host_port_ipv4(4222).await.unwrap();
    (container, port)
}

async fn nats_client(port: u16) -> async_nats::Client {
    async_nats::connect(format!("127.0.0.1:{port}"))
        .await
        .expect("Failed to connect to NATS")
}

// ── tests ─────────────────────────────────────────────────────────────────────

/// A steer message published via core NATS reaches the runner's `steer_rx`.
#[tokio::test]
async fn nats_steer_publish_reaches_runner() {
    let (_container, port) = start_nats().await;
    let nats = nats_client(port).await;

    let started = Arc::new(tokio::sync::Notify::new());
    let runner = MockAgentRunner::new("claude-test")
        .with_started_notify(started.clone())
        .with_steer_wait();

    let notifier = NatsSessionNotifier::new(nats.clone());
    let store = MemorySessionStore::new();
    let agent = TrogonAgent::new(
        notifier,
        store,
        runner.clone(),
        "acp",
        "claude-test",
        None,
        None,
        Arc::new(RwLock::new(None::<GatewayConfig>)),
    );

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async move {
            let new_resp = agent
                .new_session(NewSessionRequest::new("/cwd"))
                .await
                .unwrap();
            let session_id = new_resp.session_id.to_string();

            let steer_subject = format!("acp.session.{session_id}.agent.steer");

            let prompt_handle = tokio::task::spawn_local({
                let req = PromptRequest::new(session_id, vec![]);
                async move { agent.prompt(req).await }
            });

            // Wait until run_chat_streaming has started and is listening on steer_rx.
            tokio::time::timeout(Duration::from_secs(5), started.notified())
                .await
                .expect("runner did not start within 5 s");

            nats.publish(steer_subject, "think step by step".into())
                .await
                .unwrap();
            nats.flush().await.unwrap();

            let result = tokio::time::timeout(Duration::from_secs(5), prompt_handle)
                .await
                .expect("prompt did not complete within 5 s")
                .expect("spawn_local did not panic");
            assert!(result.is_ok(), "prompt must succeed: {result:?}");

            assert_eq!(
                runner.captured_steer(),
                vec!["think step by step"],
                "runner must have received the steer message"
            );
        })
        .await;
}

/// Multiple steer messages published in sequence all arrive at the runner.
#[tokio::test]
async fn nats_multiple_steer_messages_all_arrive() {
    let (_container, port) = start_nats().await;
    let nats = nats_client(port).await;

    let started = Arc::new(tokio::sync::Notify::new());
    let runner = MockAgentRunner::new("claude-test")
        .with_started_notify(started.clone())
        .with_steer_wait();

    let notifier = NatsSessionNotifier::new(nats.clone());
    let store = MemorySessionStore::new();
    let agent = TrogonAgent::new(
        notifier,
        store,
        runner.clone(),
        "acp",
        "claude-test",
        None,
        None,
        Arc::new(RwLock::new(None::<GatewayConfig>)),
    );

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async move {
            let new_resp = agent
                .new_session(NewSessionRequest::new("/cwd"))
                .await
                .unwrap();
            let session_id = new_resp.session_id.to_string();

            let steer_subject = format!("acp.session.{session_id}.agent.steer");

            let prompt_handle = tokio::task::spawn_local({
                let req = PromptRequest::new(session_id, vec![]);
                async move { agent.prompt(req).await }
            });

            tokio::time::timeout(Duration::from_secs(5), started.notified())
                .await
                .expect("runner did not start within 5 s");

            // Publish two messages — the runner awaits the first, then try_recv drains the second.
            nats.publish(steer_subject.clone(), "first hint".into())
                .await
                .unwrap();
            nats.publish(steer_subject.clone(), "second hint".into())
                .await
                .unwrap();
            nats.flush().await.unwrap();

            // Brief pause so the second message can be delivered before the runner
            // drains with try_recv after receiving the first.
            tokio::time::sleep(Duration::from_millis(50)).await;

            let result = tokio::time::timeout(Duration::from_secs(5), prompt_handle)
                .await
                .expect("prompt did not complete within 5 s")
                .expect("spawn_local did not panic");
            assert!(result.is_ok(), "prompt must succeed: {result:?}");

            let captured = runner.captured_steer();
            assert!(
                captured.contains(&"first hint".to_string()),
                "runner must have received 'first hint'; got: {captured:?}"
            );
            assert!(
                captured.contains(&"second hint".to_string()),
                "runner must have received 'second hint'; got: {captured:?}"
            );
        })
        .await;
}

/// A steer message for session A must not reach a prompt running for session B.
#[tokio::test]
async fn nats_steer_is_session_scoped() {
    let (_container, port) = start_nats().await;
    let nats = nats_client(port).await;

    let started = Arc::new(tokio::sync::Notify::new());
    let runner = MockAgentRunner::new("claude-test")
        .with_started_notify(started.clone())
        .with_steer_wait();

    let notifier = NatsSessionNotifier::new(nats.clone());
    let store = MemorySessionStore::new();
    let agent = TrogonAgent::new(
        notifier,
        store,
        runner.clone(),
        "acp",
        "claude-test",
        None,
        None,
        Arc::new(RwLock::new(None::<GatewayConfig>)),
    );

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async move {
            // Session A — the one running a prompt
            let resp_a = agent
                .new_session(NewSessionRequest::new("/cwd"))
                .await
                .unwrap();
            let session_a = resp_a.session_id.to_string();

            // Session B — a different session ID
            let resp_b = agent
                .new_session(NewSessionRequest::new("/cwd"))
                .await
                .unwrap();
            let session_b = resp_b.session_id.to_string();

            let steer_a = format!("acp.session.{session_a}.agent.steer");
            let steer_b = format!("acp.session.{session_b}.agent.steer");

            let prompt_handle = tokio::task::spawn_local({
                let req = PromptRequest::new(session_a.clone(), vec![]);
                async move { agent.prompt(req).await }
            });

            tokio::time::timeout(Duration::from_secs(5), started.notified())
                .await
                .expect("runner did not start within 5 s");

            // Publish to session B first (wrong session), then to session A (correct).
            nats.publish(steer_b, "wrong session".into()).await.unwrap();
            nats.publish(steer_a, "correct session".into()).await.unwrap();
            nats.flush().await.unwrap();

            let result = tokio::time::timeout(Duration::from_secs(5), prompt_handle)
                .await
                .expect("prompt did not complete within 5 s")
                .expect("spawn_local did not panic");
            assert!(result.is_ok(), "prompt must succeed: {result:?}");

            let captured = runner.captured_steer();
            assert!(
                captured.contains(&"correct session".to_string()),
                "runner must have received the steer for session A; got: {captured:?}"
            );
            assert!(
                !captured.contains(&"wrong session".to_string()),
                "runner must NOT have received the steer for session B; got: {captured:?}"
            );
        })
        .await;
}

/// A NATS publish with invalid UTF-8 bytes must not crash the agent.
/// `from_utf8_lossy` replaces invalid bytes with U+FFFD — the runner receives
/// the lossy-decoded string and the prompt completes successfully.
#[tokio::test]
async fn nats_steer_invalid_utf8_is_delivered_lossily() {
    let (_container, port) = start_nats().await;
    let nats = nats_client(port).await;

    let started = Arc::new(tokio::sync::Notify::new());
    let runner = MockAgentRunner::new("claude-test")
        .with_started_notify(started.clone())
        .with_steer_wait();

    let notifier = NatsSessionNotifier::new(nats.clone());
    let store = MemorySessionStore::new();
    let agent = TrogonAgent::new(
        notifier,
        store,
        runner.clone(),
        "acp",
        "claude-test",
        None,
        None,
        Arc::new(RwLock::new(None::<GatewayConfig>)),
    );

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async move {
            let new_resp = agent
                .new_session(NewSessionRequest::new("/cwd"))
                .await
                .unwrap();
            let session_id = new_resp.session_id.to_string();
            let steer_subject = format!("acp.session.{session_id}.agent.steer");

            let prompt_handle = tokio::task::spawn_local({
                let req = PromptRequest::new(session_id, vec![]);
                async move { agent.prompt(req).await }
            });

            tokio::time::timeout(Duration::from_secs(5), started.notified())
                .await
                .expect("runner did not start within 5 s");

            // Publish bytes that are not valid UTF-8: valid prefix + 0xFF byte.
            let mut payload = b"valid prefix ".to_vec();
            payload.push(0xFF);
            nats.publish(steer_subject, payload.into()).await.unwrap();
            nats.flush().await.unwrap();

            let result = tokio::time::timeout(Duration::from_secs(5), prompt_handle)
                .await
                .expect("prompt did not complete within 5 s")
                .expect("spawn_local did not panic");
            assert!(result.is_ok(), "prompt must succeed with invalid UTF-8 steer: {result:?}");

            let captured = runner.captured_steer();
            assert_eq!(captured.len(), 1, "runner must receive exactly one steer message");
            // from_utf8_lossy replaces 0xFF with U+FFFD (the replacement character).
            assert!(
                captured[0].contains("valid prefix"),
                "valid UTF-8 prefix must be preserved; got: {:?}",
                captured[0]
            );
            assert!(
                captured[0].contains('\u{FFFD}'),
                "invalid byte must become U+FFFD replacement character; got: {:?}",
                captured[0]
            );
        })
        .await;
}

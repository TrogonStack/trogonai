//! Integration tests for slash commands that require a real NATS connection.
//!
//! Covers the /clear flow: verifies that creating a new TrogonSession
//! (what /clear does internally) produces a distinct session_id and that
//! the new session can send prompts normally.
//!
//! Requires Docker. Run with:
//!   cargo test -p trogon-cli --test slash_commands_integration

use std::time::Duration;

use futures::StreamExt as _;
use testcontainers_modules::nats::Nats;
use testcontainers_modules::testcontainers::{ContainerAsync, runners::AsyncRunner};
use trogon_cli::session::{StreamEvent, TrogonSession};
use trogon_cli::Session as _;

// ── Helpers ───────────────────────────────────────────────────────────────────

async fn start_nats() -> (ContainerAsync<Nats>, u16) {
    let container: ContainerAsync<Nats> = Nats::default()
        .start()
        .await
        .expect("Failed to start NATS container — is Docker running?");
    let port = container.get_host_port_ipv4(4222).await.unwrap();
    (container, port)
}

async fn connect(port: u16) -> async_nats::Client {
    async_nats::connect(format!("nats://127.0.0.1:{port}"))
        .await
        .expect("connect to NATS")
}

const PREFIX: &str = "test";
const TIMEOUT: Duration = Duration::from_secs(5);

/// Spawn a fake runner that handles up to `count` session.new requests,
/// replying with incrementing session IDs: "sess-clear-1", "sess-clear-2", …
async fn spawn_fake_runner_multi(nats: async_nats::Client, base_id: &str, count: usize) {
    let subject = format!("{PREFIX}.agent.session.new");
    let mut sub = nats.subscribe(subject).await.unwrap();
    let base = base_id.to_string();
    tokio::spawn(async move {
        let mut n = 0usize;
        while n < count {
            if let Some(msg) = sub.next().await {
                if let Some(reply) = msg.reply {
                    n += 1;
                    let id = format!("{base}-{n}");
                    let body = serde_json::json!({ "sessionId": id });
                    nats.publish(reply, serde_json::to_vec(&body).unwrap().into())
                        .await
                        .ok();
                }
            }
        }
    });
}

/// Publish a Done reply to an open prompt, causing the session to close the turn.
async fn send_done(nats: &async_nats::Client, inbox: String) {
    let done = serde_json::json!({ "stopReason": "end_turn" });
    nats.publish(inbox, serde_json::to_vec(&done).unwrap().into())
        .await
        .unwrap();
}

/// Drain a receiver until Done (or channel close), with timeout.
async fn drain_until_done(mut rx: tokio::sync::mpsc::Receiver<StreamEvent>) {
    let deadline = tokio::time::sleep(TIMEOUT);
    tokio::pin!(deadline);
    loop {
        tokio::select! {
            _ = &mut deadline => panic!("timed out draining events"),
            ev = rx.recv() => match ev {
                None | Some(StreamEvent::Done(_)) => break,
                _ => {}
            }
        }
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

/// /clear creates a new session: the second TrogonSession has a different
/// session_id from the first one.
#[tokio::test]
async fn clear_creates_session_with_different_id() {
    let (_container, port) = start_nats().await;
    let nats = connect(port).await;

    spawn_fake_runner_multi(nats.clone(), "sess-clear", 2).await;

    let session1 = TrogonSession::new(nats.clone(), PREFIX, std::env::current_dir().unwrap())
        .await
        .unwrap();

    // Simulate /clear: create a fresh session on the same NATS connection.
    let session2 = TrogonSession::new(nats.clone(), PREFIX, std::env::current_dir().unwrap())
        .await
        .unwrap();

    assert_ne!(
        session1.session_id(), session2.session_id(),
        "new session after /clear must have a distinct session_id"
    );
    assert_eq!(session1.session_id(), "sess-clear-1");
    assert_eq!(session2.session_id(), "sess-clear-2");
}

/// After /clear, the new session can send prompts and receive events normally.
#[tokio::test]
async fn new_session_after_clear_can_prompt() {
    let (_container, port) = start_nats().await;
    let nats = connect(port).await;

    spawn_fake_runner_multi(nats.clone(), "sess-clear-prompt", 2).await;

    // First session — send and complete one prompt.
    let session1 = TrogonSession::new(nats.clone(), PREFIX, std::env::current_dir().unwrap())
        .await
        .unwrap();

    let resp_sub1 = format!("{PREFIX}.session.{}.agent.prompt", session1.session_id());
    let mut sub1 = nats.subscribe(resp_sub1).await.unwrap();
    let rx1 = session1.prompt("hello from session 1").await.unwrap();
    let msg1 = tokio::time::timeout(TIMEOUT, sub1.next()).await.unwrap().unwrap();
    send_done(&nats, msg1.reply.unwrap().to_string()).await;
    drain_until_done(rx1).await;

    // Simulate /clear: new session.
    let session2 = TrogonSession::new(nats.clone(), PREFIX, std::env::current_dir().unwrap())
        .await
        .unwrap();

    let resp_sub2 = format!("{PREFIX}.session.{}.agent.prompt", session2.session_id());
    let mut sub2 = nats.subscribe(resp_sub2).await.unwrap();
    let rx2 = session2.prompt("hello from session 2").await.unwrap();
    let msg2 = tokio::time::timeout(TIMEOUT, sub2.next()).await.unwrap().unwrap();

    // Verify the prompt arrived on the new session's subject.
    let payload: serde_json::Value = serde_json::from_slice(&msg2.payload).unwrap();
    assert!(
        payload.to_string().contains("hello from session 2"),
        "prompt must be routed to new session, got: {payload}"
    );

    send_done(&nats, msg2.reply.unwrap().to_string()).await;
    drain_until_done(rx2).await;
}

/// After /clear, token counters are logically reset: the new session starts
/// with no usage events until the runner sends them.
#[tokio::test]
async fn new_session_starts_with_no_usage_events() {
    let (_container, port) = start_nats().await;
    let nats = connect(port).await;

    spawn_fake_runner_multi(nats.clone(), "sess-clear-usage", 2).await;

    // First session accumulates some usage.
    let session1 = TrogonSession::new(nats.clone(), PREFIX, std::env::current_dir().unwrap())
        .await
        .unwrap();

    let notif1 = format!("{PREFIX}.session.{}.client.session.update", session1.session_id());
    let resp_sub1 = format!("{PREFIX}.session.{}.agent.prompt", session1.session_id());
    let mut sub1 = nats.subscribe(resp_sub1).await.unwrap();
    let rx1 = session1.prompt("first").await.unwrap();
    let msg1 = tokio::time::timeout(TIMEOUT, sub1.next()).await.unwrap().unwrap();

    // Emit a usage notification on the first session.
    let usage_notif = serde_json::json!({
        "sessionId": session1.session_id(),
        "update": { "sessionUpdate": "usage_update", "used": 50000, "size": 200000 }
    });
    nats.publish(notif1, serde_json::to_vec(&usage_notif).unwrap().into())
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;
    send_done(&nats, msg1.reply.unwrap().to_string()).await;
    drain_until_done(rx1).await;

    // Simulate /clear: create a fresh session.
    let session2 = TrogonSession::new(nats.clone(), PREFIX, std::env::current_dir().unwrap())
        .await
        .unwrap();

    let resp_sub2 = format!("{PREFIX}.session.{}.agent.prompt", session2.session_id());
    let mut sub2 = nats.subscribe(resp_sub2).await.unwrap();
    let mut rx2 = session2.prompt("after clear").await.unwrap();
    let msg2 = tokio::time::timeout(TIMEOUT, sub2.next()).await.unwrap().unwrap();
    // No usage notifications for the new session — just close the turn.
    send_done(&nats, msg2.reply.unwrap().to_string()).await;

    // Collect events: must not contain any Usage event.
    let mut events = Vec::new();
    let deadline = tokio::time::sleep(TIMEOUT);
    tokio::pin!(deadline);
    loop {
        tokio::select! {
            _ = &mut deadline => panic!("timed out"),
            ev = rx2.recv() => match ev {
                None | Some(StreamEvent::Done(_)) => break,
                Some(e) => events.push(e),
            }
        }
    }

    let has_usage = events.iter().any(|e| matches!(e, StreamEvent::Usage { .. }));
    assert!(
        !has_usage,
        "new session after /clear must not carry over usage events from previous session: {events:?}"
    );
}

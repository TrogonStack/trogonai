//! Integration tests for slash commands that require a real NATS connection.
//!
//! Covers /clear (session factory creates a distinct session_id), /compact
//! (export → compactor → import), and NatsSessionFactory (create + attach).
//!
//! Requires Docker. Run with:
//!   cargo test -p trogon-cli --test slash_commands_integration

use std::time::Duration;

use futures::StreamExt as _;
use testcontainers_modules::nats::Nats;
use testcontainers_modules::testcontainers::{ContainerAsync, runners::AsyncRunner};
use agent_client_protocol::ExtResponse;
use trogon_cli::session::{CompactResult, NatsSessionFactory, SessionFactory, StreamEvent, TrogonSession};
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
const REQ_ID_HEADER: &str = "X-Req-Id";

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

const COMPACT_SUBJECT: &str = "trogon.compactor.compact";

/// Respond once on `subject` with `response_bytes` (core NATS request-reply).
async fn mock_responder(nats: async_nats::Client, subject: &'static str, response_bytes: &'static [u8]) {
    let mut sub = nats.subscribe(subject).await.expect("subscribe");
    tokio::spawn(async move {
        if let Some(msg) = sub.next().await {
            if let Some(reply) = msg.reply {
                nats.publish(reply, response_bytes.into()).await.ok();
            }
        }
    });
}

fn ext_response(body: &str) -> Vec<u8> {
    let resp = ExtResponse::new(
        serde_json::value::RawValue::from_string(body.to_string())
            .unwrap()
            .into(),
    );
    serde_json::to_vec(&resp).unwrap()
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
    // Use a separate connection so same-connection echo doesn't affect delivery.
    let watcher = connect(port).await;

    spawn_fake_runner_multi(nats.clone(), "sess-clear-prompt", 2).await;

    // First session — send and complete one prompt.
    let session1 = TrogonSession::new(nats.clone(), PREFIX, std::env::current_dir().unwrap())
        .await
        .unwrap();

    let resp_sub1 = format!("{PREFIX}.session.{}.agent.prompt", session1.session_id());
    let mut sub1 = watcher.subscribe(resp_sub1).await.unwrap();
    let rx1 = session1.prompt("hello from session 1").await.unwrap();
    let msg1 = tokio::time::timeout(TIMEOUT, sub1.next()).await.unwrap().unwrap();
    // prompt() uses X-Req-Id header; derive the response subject from it.
    let req_id1 = msg1.headers.as_ref().unwrap().get(REQ_ID_HEADER).unwrap().as_str().to_string();
    let done_subj1 = format!("{PREFIX}.session.{}.agent.prompt.response.{req_id1}", session1.session_id());
    send_done(&nats, done_subj1).await;
    drain_until_done(rx1).await;

    // Simulate /clear: new session.
    let session2 = TrogonSession::new(nats.clone(), PREFIX, std::env::current_dir().unwrap())
        .await
        .unwrap();

    let resp_sub2 = format!("{PREFIX}.session.{}.agent.prompt", session2.session_id());
    let mut sub2 = watcher.subscribe(resp_sub2).await.unwrap();
    let rx2 = session2.prompt("hello from session 2").await.unwrap();
    let msg2 = tokio::time::timeout(TIMEOUT, sub2.next()).await.unwrap().unwrap();

    // Verify the prompt arrived on the new session's subject.
    let payload: serde_json::Value = serde_json::from_slice(&msg2.payload).unwrap();
    assert!(
        payload.to_string().contains("hello from session 2"),
        "prompt must be routed to new session, got: {payload}"
    );

    let req_id2 = msg2.headers.as_ref().unwrap().get(REQ_ID_HEADER).unwrap().as_str().to_string();
    let done_subj2 = format!("{PREFIX}.session.{}.agent.prompt.response.{req_id2}", session2.session_id());
    send_done(&nats, done_subj2).await;
    drain_until_done(rx2).await;
}

/// After /clear, token counters are logically reset: the new session starts
/// with no usage events until the runner sends them.
#[tokio::test]
async fn new_session_starts_with_no_usage_events() {
    let (_container, port) = start_nats().await;
    let nats = connect(port).await;
    // Use a separate connection so same-connection echo doesn't affect delivery.
    let watcher = connect(port).await;

    spawn_fake_runner_multi(nats.clone(), "sess-clear-usage", 2).await;

    // First session accumulates some usage.
    let session1 = TrogonSession::new(nats.clone(), PREFIX, std::env::current_dir().unwrap())
        .await
        .unwrap();

    let notif1 = format!("{PREFIX}.session.{}.client.session.update", session1.session_id());
    let resp_sub1 = format!("{PREFIX}.session.{}.agent.prompt", session1.session_id());
    let mut sub1 = watcher.subscribe(resp_sub1).await.unwrap();
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
    let req_id1 = msg1.headers.as_ref().unwrap().get(REQ_ID_HEADER).unwrap().as_str().to_string();
    let done_subj1 = format!("{PREFIX}.session.{}.agent.prompt.response.{req_id1}", session1.session_id());
    send_done(&nats, done_subj1).await;
    drain_until_done(rx1).await;

    // Simulate /clear: create a fresh session.
    let session2 = TrogonSession::new(nats.clone(), PREFIX, std::env::current_dir().unwrap())
        .await
        .unwrap();

    let resp_sub2 = format!("{PREFIX}.session.{}.agent.prompt", session2.session_id());
    let mut sub2 = watcher.subscribe(resp_sub2).await.unwrap();
    let mut rx2 = session2.prompt("after clear").await.unwrap();
    let msg2 = tokio::time::timeout(TIMEOUT, sub2.next()).await.unwrap().unwrap();
    // No usage notifications for the new session — just close the turn.
    let req_id2 = msg2.headers.as_ref().unwrap().get(REQ_ID_HEADER).unwrap().as_str().to_string();
    let done_subj2 = format!("{PREFIX}.session.{}.agent.prompt.response.{req_id2}", session2.session_id());
    send_done(&nats, done_subj2).await;

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

// ── NatsSessionFactory ────────────────────────────────────────────────────────

/// NatsSessionFactory::create_session opens a real NATS session and returns
/// the session_id from the runner.
#[tokio::test]
async fn nats_factory_create_session_via_real_nats() {
    let (_container, port) = start_nats().await;
    let nats = connect(port).await;

    spawn_fake_runner_multi(nats.clone(), "factory-sess", 1).await;

    let factory = NatsSessionFactory::new(nats);
    let session = factory
        .create_session(PREFIX, std::env::current_dir().unwrap(), vec![])
        .await
        .unwrap();

    assert_eq!(session.session_id(), "factory-sess-1");
}

/// NatsSessionFactory::attach_session wraps an existing session_id without
/// calling the runner — the session_id is exactly what was passed.
#[tokio::test]
async fn nats_factory_attach_session_uses_given_id() {
    let (_container, port) = start_nats().await;
    let nats = connect(port).await;

    let factory = NatsSessionFactory::new(nats);
    let session = factory.attach_session(PREFIX, "pre-migrated-session".to_string());

    assert_eq!(session.session_id(), "pre-migrated-session");
}

/// NatsSessionFactory::create_session called twice returns distinct session ids.
#[tokio::test]
async fn nats_factory_successive_creates_return_distinct_ids() {
    let (_container, port) = start_nats().await;
    let nats = connect(port).await;

    spawn_fake_runner_multi(nats.clone(), "factory-multi", 2).await;

    let factory = NatsSessionFactory::new(nats);
    let s1 = factory.create_session(PREFIX, std::env::current_dir().unwrap()).await.unwrap();
    let s2 = factory.create_session(PREFIX, std::env::current_dir().unwrap()).await.unwrap();

    assert_ne!(s1.session_id(), s2.session_id());
    assert_eq!(s1.session_id(), "factory-multi-1");
    assert_eq!(s2.session_id(), "factory-multi-2");
}

// ── /compact ──────────────────────────────────────────────────────────────────

/// session.compact() exports messages, calls trogon.compactor.compact, and imports
/// compacted messages back into the session.
#[tokio::test]
async fn compact_export_compactor_import_round_trip() {
    let (_container, port) = start_nats().await;
    let nats = connect(port).await;
    let nats_bg = connect(port).await;

    spawn_fake_runner_multi(nats_bg.clone(), "compact-sess", 1).await;
    mock_responder(
        nats_bg.clone(),
        "test.agent.ext.session/export",
        &ext_response(r#"[{"role":"user","text":"hello"}]"#),
    )
    .await;
    mock_responder(
        nats_bg.clone(),
        COMPACT_SUBJECT,
        br#"{"messages":[{"role":"user","content":[{"type":"text","text":"summary"}]}],"compacted":true,"tokens_before":500,"tokens_after":100}"#,
    )
    .await;
    mock_responder(
        nats_bg.clone(),
        "test.agent.ext.session/import",
        &ext_response("{}"),
    )
    .await;

    tokio::time::sleep(Duration::from_millis(50)).await;

    let factory = NatsSessionFactory::new(nats);
    let session = factory
        .create_session(PREFIX, std::env::current_dir().unwrap(), vec![])
        .await
        .unwrap();

    let result = session.compact().await.unwrap();
    assert_eq!(
        result,
        CompactResult {
            compacted: true,
            tokens_before: 500,
            tokens_after: 100,
        }
    );
}

/// compact() on a session created via attach_session uses the attached session id
/// when exporting and importing.
#[tokio::test]
async fn compact_on_attached_session_uses_correct_session_id() {
    let (_container, port) = start_nats().await;
    let nats = connect(port).await;
    let nats_bg = connect(port).await;

    let export_subj = "test.agent.ext.session/export";
    let import_subj = "test.agent.ext.session/import";
    let mut export_sub = nats_bg.subscribe(export_subj).await.unwrap();
    let mut import_sub = nats_bg.subscribe(import_subj).await.unwrap();
    mock_responder(
        nats_bg.clone(),
        COMPACT_SUBJECT,
        br#"{"messages":[],"compacted":false,"tokens_before":10,"tokens_after":10}"#,
    )
    .await;

    let factory = NatsSessionFactory::new(nats);
    let session = factory.attach_session(PREFIX, "attached-for-compact".to_string());

    let compact_handle = tokio::spawn(async move { session.compact().await.unwrap() });

    let export_msg = tokio::time::timeout(TIMEOUT, export_sub.next())
        .await
        .expect("timed out waiting for export")
        .expect("no export message");
    let export_req: serde_json::Value = serde_json::from_slice(&export_msg.payload).unwrap();
    assert_eq!(
        export_req["params"]["sessionId"].as_str().unwrap(),
        "attached-for-compact"
    );
    if let Some(reply) = export_msg.reply {
        nats_bg
            .publish(reply, ext_response("[]").into())
            .await
            .unwrap();
    }

    let result = compact_handle.await.unwrap();
    assert_eq!(
        result,
        CompactResult {
            compacted: false,
            tokens_before: 0,
            tokens_after: 0,
        }
    );

    // Empty export skips compactor import — import subject should not be called.
    let import_result = tokio::time::timeout(Duration::from_millis(200), import_sub.next()).await;
    assert!(import_result.is_err(), "import should not be called for empty export");
}

//! Integration tests for slash commands that require a real NATS connection.
//!
//! Covers /clear (session factory creates a distinct session_id), /compact
//! (publishes to compactor subject), and NatsSessionFactory (create + attach).
//!
//! Requires Docker. Run with:
//!   cargo test -p trogon-cli --test slash_commands_integration

use std::time::Duration;

use futures::StreamExt as _;
use testcontainers_modules::nats::Nats;
use testcontainers_modules::testcontainers::{ContainerAsync, runners::AsyncRunner};
use trogon_cli::session::{NatsSessionFactory, SessionFactory, StreamEvent, TrogonSession};
use trogon_cli::{CrossRunnerSwitcher, Session as _};

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
        .create_session(PREFIX, std::env::current_dir().unwrap())
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

/// session.compact() publishes to `{prefix}.compactor.compact` with the
/// correct session_id in the payload.
#[tokio::test]
async fn compact_publishes_to_compactor_subject_with_session_id() {
    let (_container, port) = start_nats().await;
    let nats = connect(port).await;

    // Fake runner handles one session.new
    spawn_fake_runner_multi(nats.clone(), "compact-sess", 1).await;

    // Subscribe to the compactor subject BEFORE calling compact()
    let compact_subj = format!("{PREFIX}.compactor.compact");
    let mut compact_sub = nats.subscribe(compact_subj.clone()).await.unwrap();

    let factory = NatsSessionFactory::new(nats);
    let session = factory
        .create_session(PREFIX, std::env::current_dir().unwrap())
        .await
        .unwrap();

    let expected_id = session.session_id().to_string();

    session.compact().await.unwrap();

    let msg = tokio::time::timeout(TIMEOUT, compact_sub.next())
        .await
        .expect("timed out waiting for compact message")
        .expect("compact subject had no message");

    let payload: serde_json::Value = serde_json::from_slice(&msg.payload).unwrap();
    assert_eq!(
        payload["sessionId"].as_str().unwrap(),
        expected_id,
        "compact payload must include the session_id"
    );
}

// ── /model (CrossRunnerSwitcher integration) ──────────────────────────────────

/// `CrossRunnerSwitcher::switch_model` — which backs the `/model` slash command —
/// performs the full export→new_session→import sequence via real NATS
/// request-reply and resolves the target prefix from the registry.
///
/// Uses lightweight NATS responders (no Docker containers) to exercise the
/// complete routing path without spinning up real runner processes.
#[tokio::test]
async fn model_slash_command_cross_runner_switch_via_real_nats() {
    let (_container, port) = start_nats().await;
    let nats = connect(port).await;
    let nats_bg = connect(port).await;

    // ── Mock responders for the three migration subjects ──────────────────
    // export from src runner
    {
        let n = nats_bg.clone();
        let mut sub = nats_bg
            .subscribe("acp.src.agent.ext.session/export")
            .await
            .unwrap();
        tokio::spawn(async move {
            if let Some(msg) = sub.next().await {
                if let Some(reply) = msg.reply {
                    n.publish(reply, b"[]"[..].into()).await.ok();
                }
            }
        });
    }
    // new_session on target (grok) runner
    {
        let n = nats_bg.clone();
        let mut sub = nats_bg
            .subscribe("acp.grok.agent.session.new")
            .await
            .unwrap();
        tokio::spawn(async move {
            if let Some(msg) = sub.next().await {
                if let Some(reply) = msg.reply {
                    n.publish(reply, br#"{"sessionId":"grok-sess-1"}"#[..].into())
                        .await
                        .ok();
                }
            }
        });
    }
    // session/import on target runner
    {
        let n = nats_bg.clone();
        let mut sub = nats_bg
            .subscribe("acp.grok.agent.ext.session/import")
            .await
            .unwrap();
        tokio::spawn(async move {
            if let Some(msg) = sub.next().await {
                if let Some(reply) = msg.reply {
                    n.publish(reply, b"{}"[..].into()).await.ok();
                }
            }
        });
    }

    tokio::time::sleep(Duration::from_millis(50)).await;

    // ── Registry: grok-4 → acp.grok ──────────────────────────────────────
    let registry = trogon_registry::Registry::new(trogon_registry::MockRegistryStore::new());
    let mut cap = trogon_registry::AgentCapability::new("xai", ["chat"], "acp.grok.agent.>");
    cap.metadata =
        serde_json::json!({ "models": ["grok-4"], "acp_prefix": "acp.grok" });
    registry.register(&cap).await.unwrap();

    let base_config = acp_nats::Config::new(
        acp_nats::AcpPrefix::new("acp.src").unwrap(),
        trogon_nats::NatsConfig {
            servers: vec![format!("127.0.0.1:{port}")],
            auth: trogon_nats::NatsAuth::None,
        },
    );
    let mut switcher = CrossRunnerSwitcher::new(nats, base_config, registry);

    let (new_prefix, new_session_id) = switcher
        .switch_model("acp.src", "old-session", "grok-4", "/workspace")
        .await
        .expect("switch_model must succeed via real NATS");

    assert_eq!(
        new_prefix, "acp.grok",
        "/model must route to the grok runner prefix"
    );
    assert_eq!(
        new_session_id, "grok-sess-1",
        "/model must use the session_id returned by new_session"
    );
}

/// `CrossRunnerSwitcher::switch_model` stays on the same runner when the model
/// is already served there — no migration NATS traffic is generated.
#[tokio::test]
async fn model_slash_command_same_runner_returns_unchanged_session() {
    let (_container, port) = start_nats().await;
    let nats = connect(port).await;

    let registry = trogon_registry::Registry::new(trogon_registry::MockRegistryStore::new());
    let mut cap =
        trogon_registry::AgentCapability::new("claude", ["chat", "code_edit"], "acp.acp.agent.>");
    cap.metadata = serde_json::json!({
        "models": ["claude-opus-4-6"],
        "acp_prefix": "acp.acp"
    });
    registry.register(&cap).await.unwrap();

    let base_config = acp_nats::Config::new(
        acp_nats::AcpPrefix::new("acp.acp").unwrap(),
        trogon_nats::NatsConfig {
            servers: vec![format!("127.0.0.1:{port}")],
            auth: trogon_nats::NatsAuth::None,
        },
    );
    let mut switcher = CrossRunnerSwitcher::new(nats, base_config, registry);

    let (new_prefix, new_session_id) = switcher
        .switch_model("acp.acp", "current-session", "claude-opus-4-6", "/workspace")
        .await
        .expect("same-runner switch_model must succeed");

    assert_eq!(new_prefix, "acp.acp", "prefix must be unchanged for same runner");
    assert_eq!(
        new_session_id, "current-session",
        "session_id must be unchanged for same runner"
    );
}

// ── /clear after model switch ─────────────────────────────────────────────────

/// After a `/model` cross-runner switch, `/clear` must create the new session
/// on the destination runner prefix, not the original one.
///
/// Sequence: switch_model(src→grok) → NatsSessionFactory::create_session(new_prefix) →
/// assert session routes to acp.grok, not acp.src.
#[tokio::test]
async fn clear_after_model_switch_creates_session_on_new_runner_prefix() {
    let (_container, port) = start_nats().await;
    let nats = connect(port).await;
    let nats_bg = connect(port).await;

    // Single subscriber handles BOTH the switch's new_session call AND the /clear call
    // on the grok prefix (returns "grok-sess-1" then "grok-sess-2").
    {
        let n = nats_bg.clone();
        let mut sub = nats_bg
            .subscribe("acp.grok.agent.session.new")
            .await
            .unwrap();
        tokio::spawn(async move {
            let mut count = 0usize;
            while let Some(msg) = sub.next().await {
                if let Some(reply) = msg.reply {
                    count += 1;
                    let id = format!("grok-sess-{count}");
                    let body = serde_json::json!({ "sessionId": id });
                    n.publish(reply, serde_json::to_vec(&body).unwrap().into())
                        .await
                        .ok();
                }
            }
        });
    }
    // export from src runner
    {
        let n = nats_bg.clone();
        let mut sub = nats_bg
            .subscribe("acp.src.agent.ext.session/export")
            .await
            .unwrap();
        tokio::spawn(async move {
            if let Some(msg) = sub.next().await {
                if let Some(reply) = msg.reply {
                    n.publish(reply, b"[]"[..].into()).await.ok();
                }
            }
        });
    }
    // session/import on grok runner
    {
        let n = nats_bg.clone();
        let mut sub = nats_bg
            .subscribe("acp.grok.agent.ext.session/import")
            .await
            .unwrap();
        tokio::spawn(async move {
            if let Some(msg) = sub.next().await {
                if let Some(reply) = msg.reply {
                    n.publish(reply, b"{}"[..].into()).await.ok();
                }
            }
        });
    }

    tokio::time::sleep(Duration::from_millis(50)).await;

    // Registry: grok-4 → acp.grok
    let registry = trogon_registry::Registry::new(trogon_registry::MockRegistryStore::new());
    let mut cap = trogon_registry::AgentCapability::new("xai", ["chat"], "acp.grok.agent.>");
    cap.metadata = serde_json::json!({ "models": ["grok-4"], "acp_prefix": "acp.grok" });
    registry.register(&cap).await.unwrap();

    let base_config = acp_nats::Config::new(
        acp_nats::AcpPrefix::new("acp.src").unwrap(),
        trogon_nats::NatsConfig {
            servers: vec![format!("127.0.0.1:{port}")],
            auth: trogon_nats::NatsAuth::None,
        },
    );
    let mut switcher = CrossRunnerSwitcher::new(nats.clone(), base_config, registry);

    // /model: switch from acp.src to grok-4
    let (new_prefix, _) = switcher
        .switch_model("acp.src", "old-session", "grok-4", "/workspace")
        .await
        .expect("switch_model must succeed");

    assert_eq!(new_prefix, "acp.grok", "/model must resolve to grok prefix");

    // /clear: create a new session using the prefix obtained from the switch
    let factory = NatsSessionFactory::new(nats);
    let cleared_session = factory
        .create_session(&new_prefix, std::env::current_dir().unwrap())
        .await
        .expect("/clear must create session on new runner prefix");

    assert_eq!(
        cleared_session.session_id(),
        "grok-sess-2",
        "/clear after model switch must create session on the new runner prefix (acp.grok)"
    );
}

/// compact() on a session created via attach_session also sends to the right
/// subject with the correct session_id.
#[tokio::test]
async fn compact_on_attached_session_uses_correct_session_id() {
    let (_container, port) = start_nats().await;
    let nats = connect(port).await;

    let compact_subj = format!("{PREFIX}.compactor.compact");
    let mut compact_sub = nats.subscribe(compact_subj).await.unwrap();

    let factory = NatsSessionFactory::new(nats);
    let session = factory.attach_session(PREFIX, "attached-for-compact".to_string());

    session.compact().await.unwrap();

    let msg = tokio::time::timeout(TIMEOUT, compact_sub.next())
        .await
        .expect("timed out")
        .expect("no compact message");

    let payload: serde_json::Value = serde_json::from_slice(&msg.payload).unwrap();
    assert_eq!(payload["sessionId"].as_str().unwrap(), "attached-for-compact");
}

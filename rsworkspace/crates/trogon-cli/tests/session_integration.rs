//! Integration tests for TrogonSession — verifies NATS wire format → StreamEvent mapping.
//!
//! Requires Docker (uses testcontainers to spin up a real NATS server).
//!
//! Run with:
//!   cargo test -p trogon-cli --test session_integration

use std::time::Duration;

use futures::StreamExt as _;
use testcontainers_modules::nats::Nats;
use testcontainers_modules::testcontainers::{ContainerAsync, runners::AsyncRunner};
use trogon_cli::session::{StreamEvent, TrogonSession};

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

/// Spawn a fake runner that handles exactly one `session.new` request,
/// replies with the given session_id, then drops.
async fn spawn_fake_runner(nats: async_nats::Client, session_id: String) {
    let subject = format!("{PREFIX}.agent.session.new");
    let mut sub = nats.subscribe(subject).await.unwrap();
    let id = session_id.clone();
    tokio::spawn(async move {
        if let Some(msg) = sub.next().await {
            if let Some(reply) = msg.reply {
                let body = serde_json::json!({ "sessionId": id });
                nats.publish(reply, serde_json::to_vec(&body).unwrap().into())
                    .await
                    .ok();
            }
        }
    });
}

/// Collect all StreamEvents from `rx` until Done, with a timeout.
async fn collect_events(
    mut rx: tokio::sync::mpsc::Receiver<StreamEvent>,
) -> Vec<StreamEvent> {
    let mut events = Vec::new();
    let deadline = tokio::time::sleep(TIMEOUT);
    tokio::pin!(deadline);
    loop {
        tokio::select! {
            _ = &mut deadline => panic!("timed out waiting for Done event"),
            ev = rx.recv() => match ev {
                None => break,
                Some(StreamEvent::Done(_)) => { events.push(ev.unwrap()); break; }
                Some(e) => events.push(e),
            }
        }
    }
    events
}

// ── Tests ─────────────────────────────────────────────────────────────────────

/// UsageUpdate notification → StreamEvent::Usage { used_tokens, context_size }
#[tokio::test]
async fn usage_update_notification_emits_usage_event() {
    let (_container, port) = start_nats().await;
    let nats = connect(port).await;
    let runner_nats = nats.clone();

    let session_id = "sess-usage-1".to_string();
    spawn_fake_runner(runner_nats.clone(), session_id.clone()).await;

    let session = TrogonSession::new(nats.clone(), PREFIX, std::env::current_dir().unwrap())
        .await
        .unwrap();
    assert_eq!(session.session_id, session_id);

    // Subscribe to the prompt subject to capture the request, but don't reply
    // (no reply needed — the channel closes on notif_sub drain).
    // Instead we drive the test by publishing notifications directly.

    let notif_subject = format!(
        "{PREFIX}.session.{session_id}.client.session.update"
    );
    let resp_subject = format!("{PREFIX}.session.{session_id}.agent.prompt");

    // Subscribe to catch the prompt publish so we know the inbox reply subject.
    let mut prompt_sub = nats.subscribe(resp_subject.clone()).await.unwrap();

    let rx = session.prompt("hello").await.unwrap();

    // Grab the inbox from the prompt message.
    let prompt_msg = tokio::time::timeout(TIMEOUT, prompt_sub.next())
        .await
        .expect("timeout waiting for prompt")
        .expect("prompt message");
    let inbox = prompt_msg.reply.expect("prompt must have reply inbox");

    // Publish a UsageUpdate notification.
    let usage_notif = serde_json::json!({
        "sessionId": session_id,
        "update": {
            "sessionUpdate": "usage_update",
            "used": 42000,
            "size": 200000
        }
    });
    nats.publish(
        notif_subject.clone(),
        serde_json::to_vec(&usage_notif).unwrap().into(),
    )
    .await
    .unwrap();

    // Give the session task time to receive and process the notification before
    // we publish the Done reply — the select! is biased toward resp_sub so if
    // both arrive simultaneously, Done wins and the notification is lost.
    tokio::time::sleep(Duration::from_millis(50)).await;

    // Send Done reply to close the turn.
    let done_reply = serde_json::json!({ "stopReason": "end_turn" });
    nats.publish(inbox, serde_json::to_vec(&done_reply).unwrap().into())
        .await
        .unwrap();

    let events = collect_events(rx).await;

    let usage = events.iter().find(|e| matches!(e, StreamEvent::Usage { .. }));
    assert!(usage.is_some(), "expected Usage event, got: {events:?}");
    if let Some(StreamEvent::Usage { used_tokens, context_size }) = usage {
        assert_eq!(*used_tokens, 42000);
        assert_eq!(*context_size, 200000);
    }
}

/// ToolCall(Edit) with rawInput → StreamEvent::ToolCall("Edit") + StreamEvent::Diff (colored)
#[tokio::test]
async fn tool_call_edit_emits_tool_call_and_diff_events() {
    let (_container, port) = start_nats().await;
    let nats = connect(port).await;

    let session_id = "sess-edit-1".to_string();
    spawn_fake_runner(nats.clone(), session_id.clone()).await;

    let session = TrogonSession::new(nats.clone(), PREFIX, std::env::current_dir().unwrap())
        .await
        .unwrap();

    let notif_subject = format!("{PREFIX}.session.{session_id}.client.session.update");
    let resp_subject = format!("{PREFIX}.session.{session_id}.agent.prompt");
    let mut prompt_sub = nats.subscribe(resp_subject).await.unwrap();

    let rx = session.prompt("edit something").await.unwrap();

    let prompt_msg = tokio::time::timeout(TIMEOUT, prompt_sub.next())
        .await
        .expect("timeout")
        .expect("prompt msg");
    let inbox = prompt_msg.reply.expect("inbox");

    let tool_notif = serde_json::json!({
        "sessionId": session_id,
        "update": {
            "sessionUpdate": "tool_call",
            "toolCallId": "tc-1",
            "title": "Edit",
            "rawInput": {
                "file_path": "src/main.rs",
                "old_string": "foo",
                "new_string": "bar"
            }
        }
    });
    nats.publish(notif_subject, serde_json::to_vec(&tool_notif).unwrap().into())
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;

    let done_reply = serde_json::json!({ "stopReason": "end_turn" });
    nats.publish(inbox, serde_json::to_vec(&done_reply).unwrap().into())
        .await
        .unwrap();

    let events = collect_events(rx).await;

    let tool_call = events.iter().find(|e| matches!(e, StreamEvent::ToolCall(n) if n == "Edit"));
    assert!(tool_call.is_some(), "expected ToolCall(Edit), got: {events:?}");

    let diff = events.iter().find_map(|e| {
        if let StreamEvent::Diff(d) = e { Some(d.as_str()) } else { None }
    });
    assert!(diff.is_some(), "expected Diff event after ToolCall(Edit), got: {events:?}");
    let diff = diff.unwrap();
    assert!(diff.contains("src/main.rs"), "diff must contain file path");
    assert!(diff.contains("-foo"), "diff must contain removed line");
    assert!(diff.contains("+bar"), "diff must contain added line");
}

/// ToolCall(Write) → StreamEvent::ToolCall("Write") + StreamEvent::Diff with "[write: path]"
#[tokio::test]
async fn tool_call_write_emits_tool_call_and_write_summary() {
    let (_container, port) = start_nats().await;
    let nats = connect(port).await;

    let session_id = "sess-write-1".to_string();
    spawn_fake_runner(nats.clone(), session_id.clone()).await;

    let session = TrogonSession::new(nats.clone(), PREFIX, std::env::current_dir().unwrap())
        .await
        .unwrap();

    let notif_subject = format!("{PREFIX}.session.{session_id}.client.session.update");
    let resp_subject = format!("{PREFIX}.session.{session_id}.agent.prompt");
    let mut prompt_sub = nats.subscribe(resp_subject).await.unwrap();

    let rx = session.prompt("write a file").await.unwrap();
    let prompt_msg = tokio::time::timeout(TIMEOUT, prompt_sub.next()).await.expect("timeout").expect("msg");
    let inbox = prompt_msg.reply.expect("inbox");

    let tool_notif = serde_json::json!({
        "sessionId": session_id,
        "update": {
            "sessionUpdate": "tool_call",
            "toolCallId": "tc-2",
            "title": "Write",
            "rawInput": {
                "file_path": "out.txt",
                "content": "line1\nline2\nline3"
            }
        }
    });
    nats.publish(notif_subject, serde_json::to_vec(&tool_notif).unwrap().into()).await.unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;
    nats.publish(inbox, serde_json::to_vec(&serde_json::json!({"stopReason":"end_turn"})).unwrap().into()).await.unwrap();

    let events = collect_events(rx).await;

    let tc = events.iter().find(|e| matches!(e, StreamEvent::ToolCall(n) if n == "Write"));
    assert!(tc.is_some(), "expected ToolCall(Write), got: {events:?}");

    let diff = events.iter().find_map(|e| {
        if let StreamEvent::Diff(d) = e { Some(d.as_str()) } else { None }
    });
    assert!(diff.is_some(), "expected Diff event for Write, got: {events:?}");
    let diff = diff.unwrap();
    assert!(diff.contains("out.txt"), "diff must contain path");
    assert!(diff.contains("3 lines"), "diff must contain line count");
}

/// ToolCall(Bash) → only StreamEvent::ToolCall, no Diff (Bash has no diff rendering)
#[tokio::test]
async fn tool_call_bash_emits_only_tool_call_no_diff() {
    let (_container, port) = start_nats().await;
    let nats = connect(port).await;

    let session_id = "sess-bash-1".to_string();
    spawn_fake_runner(nats.clone(), session_id.clone()).await;

    let session = TrogonSession::new(nats.clone(), PREFIX, std::env::current_dir().unwrap())
        .await
        .unwrap();

    let notif_subject = format!("{PREFIX}.session.{session_id}.client.session.update");
    let resp_subject = format!("{PREFIX}.session.{session_id}.agent.prompt");
    let mut prompt_sub = nats.subscribe(resp_subject).await.unwrap();

    let rx = session.prompt("run bash").await.unwrap();
    let prompt_msg = tokio::time::timeout(TIMEOUT, prompt_sub.next()).await.expect("timeout").expect("msg");
    let inbox = prompt_msg.reply.expect("inbox");

    let tool_notif = serde_json::json!({
        "sessionId": session_id,
        "update": {
            "sessionUpdate": "tool_call",
            "toolCallId": "tc-3",
            "title": "Bash",
            "rawInput": { "command": "ls -la" }
        }
    });
    nats.publish(notif_subject, serde_json::to_vec(&tool_notif).unwrap().into()).await.unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;
    nats.publish(inbox, serde_json::to_vec(&serde_json::json!({"stopReason":"end_turn"})).unwrap().into()).await.unwrap();

    let events = collect_events(rx).await;

    let tc = events.iter().find(|e| matches!(e, StreamEvent::ToolCall(n) if n == "Bash"));
    assert!(tc.is_some(), "expected ToolCall(Bash), got: {events:?}");

    let has_diff = events.iter().any(|e| matches!(e, StreamEvent::Diff(_)));
    assert!(!has_diff, "Bash tool must not emit a Diff event");
}

/// cancel() publishes to the correct NATS cancel subject.
#[tokio::test]
async fn cancel_publishes_to_cancel_subject() {
    let (_container, port) = start_nats().await;
    let nats = connect(port).await;

    let session_id = "sess-cancel-1".to_string();
    spawn_fake_runner(nats.clone(), session_id.clone()).await;

    let session = TrogonSession::new(nats.clone(), PREFIX, std::env::current_dir().unwrap())
        .await
        .unwrap();

    let cancel_subject = format!("{PREFIX}.session.{session_id}.agent.cancel");
    let mut cancel_sub = nats.subscribe(cancel_subject).await.unwrap();

    session.cancel().await;

    let msg = tokio::time::timeout(TIMEOUT, cancel_sub.next())
        .await
        .expect("timeout waiting for cancel message")
        .expect("cancel message");

    assert!(msg.payload.is_empty(), "cancel payload must be empty");
}

/// AgentMessageChunk notification → StreamEvent::Text
#[tokio::test]
async fn agent_message_chunk_emits_text_event() {
    let (_container, port) = start_nats().await;
    let nats = connect(port).await;

    let session_id = "sess-text-1".to_string();
    spawn_fake_runner(nats.clone(), session_id.clone()).await;

    let session = TrogonSession::new(nats.clone(), PREFIX, std::env::current_dir().unwrap())
        .await
        .unwrap();

    let notif_subject = format!("{PREFIX}.session.{session_id}.client.session.update");
    let resp_subject = format!("{PREFIX}.session.{session_id}.agent.prompt");
    let mut prompt_sub = nats.subscribe(resp_subject).await.unwrap();

    let rx = session.prompt("say hello").await.unwrap();
    let prompt_msg = tokio::time::timeout(TIMEOUT, prompt_sub.next()).await.expect("timeout").expect("msg");
    let inbox = prompt_msg.reply.expect("inbox");

    // Publish two text chunks then done.
    for chunk in &["Hello", ", world!"] {
        let notif = serde_json::json!({
            "sessionId": session_id,
            "update": {
                "sessionUpdate": "agent_message_chunk",
                "content": { "type": "text", "text": chunk }
            }
        });
        nats.publish(notif_subject.clone(), serde_json::to_vec(&notif).unwrap().into())
            .await
            .unwrap();
    }
    tokio::time::sleep(Duration::from_millis(50)).await;
    nats.publish(inbox, serde_json::to_vec(&serde_json::json!({"stopReason":"end_turn"})).unwrap().into()).await.unwrap();

    let events = collect_events(rx).await;

    let texts: Vec<&str> = events.iter().filter_map(|e| {
        if let StreamEvent::Text(t) = e { Some(t.as_str()) } else { None }
    }).collect();

    assert_eq!(texts, vec!["Hello", ", world!"], "text chunks must arrive in order: {events:?}");
}

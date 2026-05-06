//! Integration tests for non-interactive (`--print`) mode.
//!
//! Uses a real NATS container (via testcontainers) and a fake runner that
//! replies to session.new + prompt messages. Calls `print::run` directly —
//! no process spawn needed.
//!
//! Requires Docker. Run with:
//!   cargo test -p trogon-cli --test print_integration

use std::time::Duration;

use futures::StreamExt as _;
use testcontainers_modules::nats::Nats;
use testcontainers_modules::testcontainers::{ContainerAsync, runners::AsyncRunner};
use trogon_cli::session::TrogonSession;
use trogon_cli::OutputFormat;

// ── Helpers ───────────────────────────────────────────────────────────────────

const PREFIX: &str = "test";
const TIMEOUT: Duration = Duration::from_secs(5);

async fn start_nats() -> (ContainerAsync<Nats>, u16) {
    let container = Nats::default().start().await.expect("Docker required");
    let port = container.get_host_port_ipv4(4222).await.unwrap();
    (container, port)
}

async fn connect(port: u16) -> async_nats::Client {
    async_nats::connect(format!("nats://127.0.0.1:{port}"))
        .await
        .expect("connect to NATS")
}

fn cwd() -> std::path::PathBuf {
    std::env::current_dir().unwrap()
}

/// Spawn a fake runner that handles one session.new request and one prompt.
/// Replies to the prompt with `chunks` text chunks followed by a Done with
/// `stop_reason`.
async fn spawn_fake_runner(
    nats: async_nats::Client,
    session_id: &str,
    chunks: Vec<&'static str>,
    stop_reason: &'static str,
) {
    let subject_new = format!("{PREFIX}.agent.session.new");
    let mut sub_new = nats.subscribe(subject_new).await.unwrap();
    let sid = session_id.to_string();
    let nats2 = nats.clone();

    tokio::spawn(async move {
        // Handle session.new
        if let Some(msg) = sub_new.next().await {
            if let Some(reply) = msg.reply {
                let body = serde_json::json!({ "sessionId": &sid });
                nats2
                    .publish(reply, serde_json::to_vec(&body).unwrap().into())
                    .await
                    .ok();
            }
        }

        // Handle prompt
        let prompt_subj = format!("{PREFIX}.session.{sid}.agent.prompt");
        let mut sub_prompt = nats2.subscribe(prompt_subj).await.unwrap();
        if let Some(msg) = sub_prompt.next().await {
            if let Some(reply) = msg.reply {
                // Send text chunks via notifications
                let notif_subj = format!("{PREFIX}.session.{sid}.client.session.update");
                for chunk in &chunks {
                    let ev = serde_json::json!({
                        "sessionId": &sid,
                        "update": { "sessionUpdate": "message_chunk", "chunk": chunk }
                    });
                    nats2
                        .publish(notif_subj.clone(), serde_json::to_vec(&ev).unwrap().into())
                        .await
                        .ok();
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
                // Send Done
                let done = serde_json::json!({ "stopReason": stop_reason });
                nats2
                    .publish(reply, serde_json::to_vec(&done).unwrap().into())
                    .await
                    .ok();
            }
        }
    });
}

// ── Text format tests ─────────────────────────────────────────────────────────

/// The session receives Text events and Done — run() completes successfully.
#[tokio::test]
async fn print_run_streams_text_and_returns_ok() {
    let (_c, port) = start_nats().await;
    let nats = connect(port).await;

    spawn_fake_runner(nats.clone(), "sess-print-1", vec!["hello ", "world"], "end_turn").await;

    let session = TrogonSession::new(nats, PREFIX, cwd()).await.unwrap();
    let result = trogon_cli::print::run(session, "say hello", OutputFormat::Text).await;

    assert!(result.is_ok(), "expected Ok, got: {result:?}");
}

/// When the runner signals stop_reason "error", run() returns Err.
#[tokio::test]
async fn print_run_returns_err_on_error_stop_reason() {
    let (_c, port) = start_nats().await;
    let nats = connect(port).await;

    spawn_fake_runner(nats.clone(), "sess-print-err", vec![], "error").await;

    let session = TrogonSession::new(nats, PREFIX, cwd()).await.unwrap();
    let result = trogon_cli::print::run(session, "trigger error", OutputFormat::Text).await;

    assert!(result.is_err(), "expected Err for stop_reason=error");
    let msg = result.unwrap_err().to_string();
    assert!(msg.contains("error"), "got: {msg}");
}

/// A prompt sent in print mode reaches the correct NATS subject.
#[tokio::test]
async fn print_run_prompt_routed_to_session_subject() {
    let (_c, port) = start_nats().await;
    let nats = connect(port).await;

    let subject_new = format!("{PREFIX}.agent.session.new");
    let mut sub_new = nats.subscribe(subject_new).await.unwrap();
    let nats2 = nats.clone();
    let sid = "sess-print-route";

    let (tx, mut rx) = tokio::sync::mpsc::channel::<serde_json::Value>(1);

    tokio::spawn(async move {
        if let Some(msg) = sub_new.next().await {
            if let Some(reply) = msg.reply {
                let body = serde_json::json!({ "sessionId": sid });
                nats2
                    .publish(reply, serde_json::to_vec(&body).unwrap().into())
                    .await
                    .ok();
            }
        }
        let prompt_subj = format!("{PREFIX}.session.{sid}.agent.prompt");
        let mut sub_p = nats2.subscribe(prompt_subj).await.unwrap();
        if let Some(msg) = sub_p.next().await {
            let payload: serde_json::Value =
                serde_json::from_slice(&msg.payload).unwrap_or_default();
            tx.send(payload.clone()).await.ok();
            if let Some(reply) = msg.reply {
                let done = serde_json::json!({ "stopReason": "end_turn" });
                nats2
                    .publish(reply, serde_json::to_vec(&done).unwrap().into())
                    .await
                    .ok();
            }
        }
    });

    let session = TrogonSession::new(nats, PREFIX, cwd()).await.unwrap();
    trogon_cli::print::run(session, "unique-sentinel-prompt", OutputFormat::Text).await.ok();

    let payload = tokio::time::timeout(TIMEOUT, rx.recv())
        .await
        .expect("timed out")
        .expect("no payload");
    assert!(
        payload.to_string().contains("unique-sentinel-prompt"),
        "prompt not in payload: {payload}"
    );
}

/// max_tokens stop reason is treated as success (not an error).
#[tokio::test]
async fn print_run_max_tokens_is_not_an_error() {
    let (_c, port) = start_nats().await;
    let nats = connect(port).await;

    spawn_fake_runner(nats.clone(), "sess-print-maxtok", vec!["partial"], "max_tokens").await;

    let session = TrogonSession::new(nats, PREFIX, cwd()).await.unwrap();
    let result = trogon_cli::print::run(session, "write a lot", OutputFormat::Text).await;

    assert!(result.is_ok(), "max_tokens should not be an error: {result:?}");
}

/// Session isolation: two consecutive print::run calls get distinct session IDs.
#[tokio::test]
async fn print_run_two_calls_get_distinct_session_ids() {
    let (_c, port) = start_nats().await;
    let nats = connect(port).await;

    let subj = format!("{PREFIX}.agent.session.new");
    let mut sub = nats.subscribe(subj).await.unwrap();
    let nats2 = nats.clone();
    tokio::spawn(async move {
        let mut n = 0usize;
        while n < 2 {
            if let Some(msg) = sub.next().await {
                if let Some(reply) = msg.reply {
                    n += 1;
                    let body = serde_json::json!({ "sessionId": format!("sess-iso-{n}") });
                    nats2
                        .publish(reply, serde_json::to_vec(&body).unwrap().into())
                        .await
                        .ok();
                    let sid = format!("sess-iso-{n}");
                    let ps = format!("{PREFIX}.session.{sid}.agent.prompt");
                    let mut sp = nats2.subscribe(ps).await.unwrap();
                    if let Some(pm) = sp.next().await {
                        if let Some(r) = pm.reply {
                            let done = serde_json::json!({ "stopReason": "end_turn" });
                            nats2
                                .publish(r, serde_json::to_vec(&done).unwrap().into())
                                .await
                                .ok();
                        }
                    }
                }
            }
        }
    });

    let s1 = TrogonSession::new(nats.clone(), PREFIX, cwd()).await.unwrap();
    trogon_cli::print::run(s1, "first", OutputFormat::Text).await.unwrap();
    let s2 = TrogonSession::new(nats.clone(), PREFIX, cwd()).await.unwrap();
    trogon_cli::print::run(s2, "second", OutputFormat::Text).await.unwrap();
}

// ── JSON format tests ─────────────────────────────────────────────────────────

/// JSON mode completes successfully and returns Ok (stdout content is a JSON line).
#[tokio::test]
async fn print_run_json_format_returns_ok_on_success() {
    let (_c, port) = start_nats().await;
    let nats = connect(port).await;

    spawn_fake_runner(
        nats.clone(),
        "sess-json-ok",
        vec!["hello ", "world"],
        "end_turn",
    )
    .await;

    let session = TrogonSession::new(nats, PREFIX, cwd()).await.unwrap();
    let result = trogon_cli::print::run(session, "say hello", OutputFormat::Json).await;

    assert!(result.is_ok(), "expected Ok in json mode, got: {result:?}");
}

/// JSON mode also returns Err when the runner signals stop_reason "error".
#[tokio::test]
async fn print_run_json_format_error_stop_reason_returns_err() {
    let (_c, port) = start_nats().await;
    let nats = connect(port).await;

    spawn_fake_runner(nats.clone(), "sess-json-err", vec![], "error").await;

    let session = TrogonSession::new(nats, PREFIX, cwd()).await.unwrap();
    let result = trogon_cli::print::run(session, "trigger error", OutputFormat::Json).await;

    assert!(result.is_err(), "expected Err in json mode for stop_reason=error");
}

/// JSON mode: max_tokens is not an error.
#[tokio::test]
async fn print_run_json_format_max_tokens_is_not_an_error() {
    let (_c, port) = start_nats().await;
    let nats = connect(port).await;

    spawn_fake_runner(
        nats.clone(),
        "sess-json-maxtok",
        vec!["partial text"],
        "max_tokens",
    )
    .await;

    let session = TrogonSession::new(nats, PREFIX, cwd()).await.unwrap();
    let result = trogon_cli::print::run(session, "write a lot", OutputFormat::Json).await;

    assert!(result.is_ok(), "max_tokens should not be an error in json mode: {result:?}");
}

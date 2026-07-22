#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! LIVE: the IDE's ACP-over-NATS protocol (originally built for the Claude /
//! acp-runner) drives the **xai-runner** end-to-end against the REAL xAI API.
//!
//! This is the "multi-runner IDE" proof. The IDE side (`acp_nats::Bridge`) talks
//! to every runner through the same ACP-over-NATS wire protocol:
//!
//!   * `acp.agent.initialize`                    (global)
//!   * `acp.agent.session.new`                   (global)
//!   * `acp.session.{id}.agent.prompt`           (session-scoped, == Bridge `PromptSubject`)
//!   * `acp.session.{id}.client.session.update`  (streamed assistant output)
//!
//! Here we stand up the REAL xai-runner serve loop (`XaiAgent` with the real
//! `XaiClient` + a real key + grok-3) behind a real NATS server, then drive it
//! with exactly those subjects — the same the IDE Bridge publishes — and assert
//! a real turn completes. No mock client: reaching a `stopReason` is only
//! possible if xAI actually answered.
//!
//! `#[ignore]` by default — never runs in CI. Requires `XAI_API_KEY` + Docker.
//! Run with:
//!   cargo test -p trogon-xai-runner --test live_multi_runner_ide -- --ignored --nocapture

use std::sync::Arc;
use std::time::Duration;

use acp_nats::AcpPrefix;
use acp_nats_agent::AgentSideNatsConnection;
use futures::StreamExt as _;
use serde_json::{Value, json};
use testcontainers_modules::nats::Nats;
use testcontainers_modules::testcontainers::{ContainerAsync, ImageExt, runners::AsyncRunner};
use tokio::sync::Mutex;
use trogon_xai_runner::{NatsSessionNotifier, XaiAgent};

async fn start_nats() -> (ContainerAsync<Nats>, async_nats::Client) {
    let container = Nats::default()
        .with_cmd(["--jetstream"])
        .start()
        .await
        .expect("Failed to start NATS container — is Docker running?");
    let port = container.get_host_port_ipv4(4222).await.unwrap();
    let nats = async_nats::connect(format!("127.0.0.1:{port}"))
        .await
        .expect("connect to NATS");
    (container, nats)
}

/// Spawn the REAL xai-runner serve loop (real `XaiClient`, real key, grok-3) in
/// a dedicated thread, exactly as `main.rs` does (core ACP-over-NATS serve loop).
async fn start_live_runner(nats: async_nats::Client, api_key: String) {
    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let local = tokio::task::LocalSet::new();
        let prefix = AcpPrefix::new("acp").unwrap();
        let notifier = NatsSessionNotifier::new(nats.clone(), prefix.clone());
        // `XaiAgent::new` builds the real `XaiClient` — no mock. grok-3 is cheap.
        let agent = XaiAgent::new(notifier, "grok-3", api_key);
        let (_, io_task) = AgentSideNatsConnection::new(agent, nats, prefix, |fut| {
            tokio::task::spawn_local(fut);
        });
        rt.block_on(local.run_until(async move {
            io_task.await.ok();
        }));
    });
    // Give the runner time to subscribe before the IDE sends requests.
    tokio::time::sleep(Duration::from_millis(400)).await;
}

#[ignore = "requires XAI_API_KEY — hits the real xAI API (one short grok-3 prompt)"]
#[tokio::test]
async fn multi_runner_ide_drives_xai_runner_live() {
    let api_key = std::env::var("XAI_API_KEY").expect("XAI_API_KEY must be set for the live test");

    let (_container, nats) = start_nats().await;
    start_live_runner(nats.clone(), api_key).await;

    // ── IDE step 1: initialize (global subject, built for Claude/acp) ──────────
    let init = nats
        .request(
            "acp.agent.initialize",
            serde_json::to_vec(&json!({ "protocolVersion": 0 })).unwrap().into(),
        )
        .await
        .expect("initialize over NATS");
    let init: Value = serde_json::from_slice(&init.payload).unwrap();
    assert!(
        init["agentCapabilities"].is_object(),
        "xai-runner must answer the IDE's initialize: {init}"
    );

    // ── IDE step 2: session/new ────────────────────────────────────────────────
    let new = nats
        .request(
            "acp.agent.session.new",
            serde_json::to_vec(&json!({
                "sessionId": null,
                "cwd": "/tmp",
                "mcpServers": []
            }))
            .unwrap()
            .into(),
        )
        .await
        .expect("session/new over NATS");
    let new: Value = serde_json::from_slice(&new.payload).unwrap();
    let session_id = new["sessionId"].as_str().unwrap_or("").to_string();
    assert!(!session_id.is_empty(), "must get a session id: {new}");
    eprintln!("IDE opened session on xai-runner: {session_id}");

    // ── IDE subscribes to streamed assistant output (same as the Bridge) ───────
    let updates = Arc::new(Mutex::new(Vec::<Value>::new()));
    let mut sub = nats
        .subscribe(format!("acp.session.{session_id}.client.session.update"))
        .await
        .expect("subscribe to session updates");
    let updates_collector = updates.clone();
    let collector = tokio::spawn(async move {
        while let Some(msg) = sub.next().await {
            if let Ok(v) = serde_json::from_slice::<Value>(&msg.payload) {
                updates_collector.lock().await.push(v);
            }
        }
    });

    // ── IDE step 3: prompt (session-scoped subject == Bridge PromptSubject) ─────
    let prompt_subject = format!("acp.session.{session_id}.agent.prompt");
    let prompt_payload = serde_json::to_vec(&json!({
        "sessionId": session_id,
        "prompt": [ { "type": "text", "text": "Reply with exactly: PONG" } ]
    }))
    .unwrap();

    let resp = tokio::time::timeout(
        Duration::from_secs(90),
        nats.request(prompt_subject, prompt_payload.into()),
    )
    .await
    .expect("prompt timed out — real xAI call too slow")
    .expect("prompt over NATS");
    let resp: Value = serde_json::from_slice(&resp.payload).unwrap();
    eprintln!("\n=== PromptResponse from xai-runner ===\n{resp}\n");

    // Give any trailing notifications a moment to land, then stop collecting.
    tokio::time::sleep(Duration::from_millis(300)).await;
    collector.abort();
    let collected = updates.lock().await;

    // Print the streamed assistant text as evidence of a real round-trip.
    let mut texts = Vec::new();
    for u in collected.iter() {
        collect_text(u, &mut texts);
    }
    eprintln!("=== streamed session updates ({}) ===", collected.len());
    for t in &texts {
        eprintln!("  chunk: {t}");
    }

    // The response can only carry a `stopReason` if a real xAI turn completed —
    // there is no mock client in this test.
    let stop = resp["stopReason"].as_str();
    assert!(
        stop.is_some(),
        "xai-runner must complete a real turn and return a stopReason; got: {resp}"
    );
    eprintln!(
        "\n✅ multi-runner IDE → xai-runner LIVE: stopReason = {} ({} update notifications)",
        stop.unwrap(),
        collected.len()
    );
    assert!(
        !collected.is_empty(),
        "the IDE must receive streamed session.update notifications from the xai-runner"
    );
}

/// Recursively pull any `"text"` string fields out of a session-update payload.
fn collect_text(v: &Value, out: &mut Vec<String>) {
    match v {
        Value::Object(map) => {
            for (k, val) in map {
                if k == "text"
                    && let Some(s) = val.as_str()
                    && !s.is_empty()
                {
                    out.push(s.to_string());
                }
                collect_text(val, out);
            }
        }
        Value::Array(arr) => {
            for item in arr {
                collect_text(item, out);
            }
        }
        _ => {}
    }
}

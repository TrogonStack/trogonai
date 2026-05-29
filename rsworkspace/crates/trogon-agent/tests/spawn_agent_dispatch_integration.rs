//! Integration tests for `spawn_agent::dispatch` — requires Docker.
//!
//! Run with:
//!   cargo test -p trogon-agent --test spawn_agent_dispatch_integration

use futures_util::StreamExt as _;
use testcontainers_modules::nats::Nats;
use testcontainers_modules::testcontainers::{ContainerAsync, ImageExt, runners::AsyncRunner};
use trogon_agent::tools::spawn_agent;

async fn start_nats() -> (ContainerAsync<Nats>, async_nats::Client) {
    let c = Nats::default()
        .with_cmd(["--jetstream"])
        .start()
        .await
        .expect("Failed to start NATS container — is Docker running?");
    let port = c.get_host_port_ipv4(4222).await.unwrap();
    let nats = async_nats::connect(format!("127.0.0.1:{port}"))
        .await
        .expect("connect to NATS");
    (c, nats)
}

fn spawn_responder(nats: async_nats::Client, subject: String, reply_body: &'static str) {
    tokio::spawn(async move {
        let mut sub = nats.subscribe(subject).await.unwrap();
        if let Some(msg) = sub.next().await {
            if let Some(reply) = msg.reply {
                nats.publish(reply, reply_body.into()).await.unwrap();
            }
        }
    });
}

// ── tests ─────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn dispatch_returns_responder_output() {
    let (_c, nats) = start_nats().await;
    let prefix = "trogon";

    spawn_responder(
        nats.clone(),
        format!("{prefix}.agent.spawn"),
        "Plan agent: here is the plan",
    );
    tokio::time::sleep(std::time::Duration::from_millis(30)).await;

    let result = spawn_agent::dispatch(&nats, prefix, "plan", "design the cache layer")
        .await
        .unwrap();

    assert!(
        result.contains("Plan agent"),
        "expected responder output, got: {result}"
    );
}

#[tokio::test]
async fn dispatch_sends_capability_and_prompt_in_payload() {
    let (_c, nats) = start_nats().await;
    let prefix = "trogon2";

    let nats_clone = nats.clone();
    tokio::spawn(async move {
        let mut sub = nats_clone
            .subscribe(format!("{prefix}.agent.spawn"))
            .await
            .unwrap();
        if let Some(msg) = sub.next().await {
            let body: serde_json::Value =
                serde_json::from_slice(&msg.payload).unwrap_or_default();
            let echo = serde_json::to_string(&body).unwrap();
            if let Some(reply) = msg.reply {
                nats_clone.publish(reply, echo.into()).await.unwrap();
            }
        }
    });
    tokio::time::sleep(std::time::Duration::from_millis(30)).await;

    let result = spawn_agent::dispatch(&nats, prefix, "explore", "list all endpoints")
        .await
        .unwrap();

    let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();
    assert_eq!(parsed["capability"].as_str(), Some("explore"));
    assert_eq!(parsed["prompt"].as_str(), Some("list all endpoints"));
}

#[tokio::test]
async fn dispatch_errors_when_no_responder() {
    let (_c, nats) = start_nats().await;
    let result = spawn_agent::dispatch(&nats, "no-responder", "explore", "anything").await;
    assert!(result.is_err(), "must fail when no agent is subscribed");
}

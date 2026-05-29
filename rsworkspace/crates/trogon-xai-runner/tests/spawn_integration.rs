//! Integration tests for the spawn-handler NATS subscriber (PR 16).
//!
//! Spins up a real NATS container and exercises `run_spawn_subscriber` with a
//! mock HTTP client so no real xAI API key is needed.
//!
//! Run with:
//!   cargo test -p trogon-xai-runner --test spawn_integration

use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use testcontainers_modules::nats::Nats;
use testcontainers_modules::testcontainers::{ImageExt, runners::AsyncRunner};
use trogon_xai_runner::spawn_handler::{SpawnHttpClient, run_spawn_subscriber};

// ── helpers ───────────────────────────────────────────────────────────────────

async fn start_nats() -> (impl Drop, async_nats::Client) {
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

// ── Mock HTTP client ──────────────────────────────────────────────────────────

struct MockSpawnClient {
    response: String,
    captured_prompt: Arc<Mutex<Option<String>>>,
}

impl MockSpawnClient {
    fn new(
        response: impl Into<String>,
    ) -> (Arc<Self>, Arc<Mutex<Option<String>>>) {
        let captured = Arc::new(Mutex::new(None));
        (
            Arc::new(Self { response: response.into(), captured_prompt: Arc::clone(&captured) }),
            captured,
        )
    }
}

#[async_trait]
impl SpawnHttpClient for MockSpawnClient {
    async fn complete(&self, _api_key: &str, _model: &str, prompt: &str, _url: &str) -> String {
        *self.captured_prompt.lock().unwrap() = Some(prompt.to_string());
        self.response.clone()
    }
}

// ── tests ─────────────────────────────────────────────────────────────────────

/// Happy path: a spawn request with a prompt is replied to with the mock result.
#[tokio::test]
async fn spawn_subscriber_replies_with_http_client_result() {
    let (_c, nats) = start_nats().await;
    let (client, _) = MockSpawnClient::new("agent result text");

    tokio::spawn(run_spawn_subscriber(
        nats.clone(),
        "acp".to_string(),
        "key".to_string(),
        "grok-4".to_string(),
        "https://api.x.ai/v1".to_string(),
        client,
    ));
    tokio::time::sleep(Duration::from_millis(200)).await;

    let payload = serde_json::to_vec(&serde_json::json!({ "prompt": "explain recursion" })).unwrap();
    let reply = tokio::time::timeout(
        Duration::from_secs(5),
        nats.request("acp.agent.spawn", payload.into()),
    )
    .await
    .expect("timed out waiting for spawn reply")
    .expect("NATS request failed");

    assert_eq!(
        std::str::from_utf8(&reply.payload).unwrap(),
        "agent result text"
    );
}

/// The subscriber extracts the prompt from the JSON payload and passes it to the client.
#[tokio::test]
async fn spawn_subscriber_passes_prompt_to_http_client() {
    let (_c, nats) = start_nats().await;
    let (client, captured_prompt) = MockSpawnClient::new("ok");

    tokio::spawn(run_spawn_subscriber(
        nats.clone(),
        "acp".to_string(),
        "key".to_string(),
        "grok-4".to_string(),
        "https://api.x.ai/v1".to_string(),
        client,
    ));
    tokio::time::sleep(Duration::from_millis(200)).await;

    let payload = serde_json::to_vec(&serde_json::json!({ "prompt": "what is a monad?" })).unwrap();
    tokio::time::timeout(
        Duration::from_secs(5),
        nats.request("acp.agent.spawn", payload.into()),
    )
    .await
    .expect("timed out")
    .expect("NATS request failed");

    assert_eq!(
        captured_prompt.lock().unwrap().as_deref(),
        Some("what is a monad?")
    );
}

/// A message without a reply subject is skipped and the subscriber keeps running.
#[tokio::test]
async fn spawn_subscriber_skips_message_without_reply_and_keeps_running() {
    let (_c, nats) = start_nats().await;
    let (client, _) = MockSpawnClient::new("ok");

    tokio::spawn(run_spawn_subscriber(
        nats.clone(),
        "acp".to_string(),
        "key".to_string(),
        "grok-4".to_string(),
        "https://api.x.ai/v1".to_string(),
        client,
    ));
    tokio::time::sleep(Duration::from_millis(200)).await;

    // Publish without reply subject — subscriber must not crash.
    nats.publish(
        "acp.agent.spawn",
        serde_json::to_vec(&serde_json::json!({ "prompt": "hi" })).unwrap().into(),
    )
    .await
    .unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Verify the subscriber is still running by sending a proper request.
    let payload = serde_json::to_vec(&serde_json::json!({ "prompt": "still alive?" })).unwrap();
    let reply = tokio::time::timeout(
        Duration::from_secs(5),
        nats.request("acp.agent.spawn", payload.into()),
    )
    .await
    .expect("subscriber must still be alive after no-reply message")
    .expect("NATS request failed");

    assert!(!reply.payload.is_empty());
}

/// A message with invalid JSON is skipped and the subscriber keeps running.
#[tokio::test]
async fn spawn_subscriber_skips_invalid_json_and_keeps_running() {
    let (_c, nats) = start_nats().await;
    let (client, _) = MockSpawnClient::new("ok");

    tokio::spawn(run_spawn_subscriber(
        nats.clone(),
        "acp".to_string(),
        "key".to_string(),
        "grok-4".to_string(),
        "https://api.x.ai/v1".to_string(),
        client,
    ));
    tokio::time::sleep(Duration::from_millis(200)).await;

    // Send invalid JSON — must not crash. We use publish (no reply) then request.
    nats.publish("acp.agent.spawn", b"not json at all"[..].into())
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Verify still alive.
    let payload = serde_json::to_vec(&serde_json::json!({ "prompt": "still alive?" })).unwrap();
    let reply = tokio::time::timeout(
        Duration::from_secs(5),
        nats.request("acp.agent.spawn", payload.into()),
    )
    .await
    .expect("subscriber must still be alive after invalid JSON message")
    .expect("NATS request failed");

    assert!(!reply.payload.is_empty());
}

/// Missing prompt field defaults to empty string passed to the HTTP client.
#[tokio::test]
async fn spawn_subscriber_defaults_to_empty_prompt_when_field_missing() {
    let (_c, nats) = start_nats().await;
    let (client, captured_prompt) = MockSpawnClient::new("ok");

    tokio::spawn(run_spawn_subscriber(
        nats.clone(),
        "acp".to_string(),
        "key".to_string(),
        "grok-4".to_string(),
        "https://api.x.ai/v1".to_string(),
        client,
    ));
    tokio::time::sleep(Duration::from_millis(200)).await;

    let payload = serde_json::to_vec(&serde_json::json!({ "other": "field" })).unwrap();
    tokio::time::timeout(
        Duration::from_secs(5),
        nats.request("acp.agent.spawn", payload.into()),
    )
    .await
    .expect("timed out")
    .expect("NATS request failed");

    assert_eq!(
        captured_prompt.lock().unwrap().as_deref(),
        Some("")
    );
}

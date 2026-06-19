//! End-to-end tests for the compactor NATS service (`service::run`).
//!
//! Verifies the full request-reply boundary: a caller publishes to
//! `trogon.compactor.compact`, the service compacts the conversation and
//! replies with the result. The LLM call goes to a local `httpmock` server
//! returning a fixed Anthropic-format summary — no real key needed.
//!
//! Requires Docker (testcontainers starts a NATS server).
//!
//! Run with:
//!   cargo test -p trogon-compactor --test service_e2e

use testcontainers_modules::nats::Nats;
use testcontainers_modules::testcontainers::{ImageExt, runners::AsyncRunner};
use trogon_compactor::{
    AuthStyle, CompactionSettings, Message,
    service::{self, ProviderConfig, ServiceState},
};

// ── helpers ───────────────────────────────────────────────────────────────────

fn small_settings() -> CompactionSettings {
    CompactionSettings {
        context_window: 100,
        reserve_tokens: 10,
        keep_recent_tokens: 20,
    }
}

fn large_conversation() -> Vec<Message> {
    (0..4)
        .flat_map(|i| {
            [
                Message::user(format!("question {i}: {}", "q".repeat(200))),
                Message::assistant(format!("answer {i}: {}", "a".repeat(200))),
            ]
        })
        .collect()
}

/// A request payload as raw JSON (only `messages` — exercises the backward-compat
/// path where provider/model are absent and default to anthropic).
fn compact_payload(messages: &[Message]) -> Vec<u8> {
    serde_json::to_vec(&serde_json::json!({ "messages": messages })).unwrap()
}

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

/// Build a ServiceState whose anthropic provider points at `api_url` (a mock
/// server returning an Anthropic `/v1/messages` response).
fn state_with_anthropic(api_url: String) -> ServiceState {
    ServiceState {
        client: reqwest::Client::new(),
        default_settings: small_settings(),
        max_summary_tokens: 1_000,
        anthropic: Some(ProviderConfig {
            api_url,
            token: "tok_test".into(),
            auth_style: AuthStyle::Bearer,
            default_model: "claude-test".into(),
        }),
        xai: None,
        openrouter: None,
    }
}

/// Start an httpmock server that answers the Anthropic messages endpoint with a
/// fixed summary, returning its `/v1/messages` URL.
async fn start_anthropic_mock() -> (httpmock::MockServer, String) {
    let server = httpmock::MockServer::start_async().await;
    server
        .mock_async(|when, then| {
            when.method("POST").path("/v1/messages");
            then.status(200)
                .header("content-type", "application/json")
                .json_body(serde_json::json!({
                    "stop_reason": "end_turn",
                    "content": [{ "type": "text", "text": "## Summary\nCompacted context" }]
                }));
        })
        .await;
    let url = format!("{}/v1/messages", server.base_url());
    (server, url)
}

/// ServiceState with an xAI provider (OpenAI-compatible) pointing at `api_url`.
fn state_with_xai(api_url: String) -> ServiceState {
    ServiceState {
        client: reqwest::Client::new(),
        default_settings: small_settings(),
        max_summary_tokens: 1_000,
        anthropic: None,
        xai: Some(ProviderConfig {
            api_url,
            token: "tok_xai".into(),
            auth_style: AuthStyle::Bearer,
            default_model: "grok-default".into(),
        }),
        openrouter: None,
    }
}

/// Start an httpmock that answers an OpenAI-compatible `/chat/completions`
/// endpoint, but ONLY when the request body carries `expect_model`. This makes
/// the test assert the service resolved the right model (override precedence):
/// if the service sent the wrong model, the mock won't match → no compaction.
async fn start_openai_compat_mock(expect_model: &str) -> (httpmock::MockServer, String) {
    let server = httpmock::MockServer::start_async().await;
    let needle = format!("\"model\":\"{expect_model}\"");
    server
        .mock_async(move |when, then| {
            when.method("POST").path("/v1/chat/completions").body_contains(needle);
            then.status(200)
                .header("content-type", "application/json")
                .json_body(serde_json::json!({
                    "choices": [{
                        "message": { "content": "## Summary\nOpenAI-compat compacted" },
                        "finish_reason": "stop"
                    }]
                }));
        })
        .await;
    let url = format!("{}/v1/chat/completions", server.base_url());
    (server, url)
}

// ── tests ─────────────────────────────────────────────────────────────────────

/// The compactor service subscribes to `trogon.compactor.compact`, compacts a
/// large conversation, and replies with `compacted=true` and fewer messages.
#[tokio::test]
async fn compactor_service_responds_to_nats_compact_request() {
    let (_container, nats) = start_nats().await;
    let (_mock, api_url) = start_anthropic_mock().await;

    let nats_for_service = nats.clone();
    tokio::spawn(async move {
        service::run(nats_for_service, state_with_anthropic(api_url)).await.ok();
    });
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let messages = large_conversation();
    let original_len = messages.len();
    let payload = compact_payload(&messages);

    let reply = nats
        .request(service::COMPACT_SUBJECT, payload.into())
        .await
        .expect("NATS request to compactor must succeed");

    let resp: serde_json::Value = serde_json::from_slice(&reply.payload).expect("response must be valid JSON");

    assert_eq!(
        resp["compacted"].as_bool(),
        Some(true),
        "expected compacted=true; got: {resp}"
    );
    let after_len = resp["messages"]
        .as_array()
        .map(|a| a.len())
        .expect("messages must be an array");
    assert!(
        after_len < original_len,
        "expected fewer messages after compaction ({after_len} < {original_len})"
    );
    // kept_count must be present and consistent (Gap 2).
    assert!(
        resp["kept_count"].as_u64().is_some(),
        "expected kept_count in response; got: {resp}"
    );
    let first_text = resp["messages"][0]["content"][0]["text"].as_str().unwrap_or("");
    assert!(
        first_text.contains("context-summary"),
        "expected <context-summary> tag in first compacted message; got: {first_text}"
    );
}

/// A fire-and-forget publish (no reply subject) must be silently discarded.
/// The service must continue handling subsequent valid requests.
#[tokio::test]
async fn compactor_service_ignores_fire_and_forget_and_keeps_serving() {
    let (_container, nats) = start_nats().await;
    let (_mock, api_url) = start_anthropic_mock().await;

    let nats_for_service = nats.clone();
    tokio::spawn(async move {
        service::run(nats_for_service, state_with_anthropic(api_url)).await.ok();
    });
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let payload = compact_payload(&large_conversation());

    // Fire-and-forget: publish without a reply subject — service must not crash.
    nats.publish(service::COMPACT_SUBJECT, payload.clone().into())
        .await
        .unwrap();

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    // Service must still respond correctly to a subsequent valid request.
    let reply = nats
        .request(service::COMPACT_SUBJECT, payload.into())
        .await
        .expect("service must still respond after fire-and-forget");

    let resp: serde_json::Value = serde_json::from_slice(&reply.payload).expect("response must be valid JSON");

    assert!(
        resp.get("messages").is_some(),
        "expected valid response after fire-and-forget; got: {resp}"
    );
}

/// Multi-provider: a `provider:"xai"` request routes to the OpenAI-compatible
/// `/chat/completions` path, and the `compactor_model` override is what the
/// service actually sends (the mock only matches the override model — if the
/// service used the wrong model, no compaction would happen).
#[tokio::test]
async fn compactor_service_uses_openai_compat_provider_and_model_override() {
    let (_container, nats) = start_nats().await;
    // Mock only answers when the body carries the OVERRIDE model "grok-3-mini".
    let (_mock, api_url) = start_openai_compat_mock("grok-3-mini").await;

    let nats_for_service = nats.clone();
    tokio::spawn(async move {
        service::run(nats_for_service, state_with_xai(api_url)).await.ok();
    });
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let messages = large_conversation();
    // provider=xai, session model=grok-3, override=grok-3-mini.
    let payload = serde_json::to_vec(&serde_json::json!({
        "messages": messages,
        "provider": "xai",
        "model": "grok-3",
        "compactor_model": "grok-3-mini",
        "context_window": 100,
    }))
    .unwrap();

    let reply = nats
        .request(service::COMPACT_SUBJECT, payload.into())
        .await
        .expect("NATS request must succeed");
    let resp: serde_json::Value = serde_json::from_slice(&reply.payload).expect("valid JSON");

    // compacted=true proves: routed to OpenAI-compat AND used the override model
    // (the mock only matches "grok-3-mini").
    assert_eq!(
        resp["compacted"].as_bool(),
        Some(true),
        "xai request must compact via OpenAI-compat with the override model; got: {resp}"
    );
    let first_text = resp["messages"][0]["content"][0]["text"].as_str().unwrap_or("");
    assert!(
        first_text.contains("OpenAI-compat compacted"),
        "summary must come from the OpenAI-compat mock; got: {first_text}"
    );

    // kept_count must be consistent: output = summary + ack + kept tail.
    let kept = resp["kept_count"].as_u64().expect("kept_count present") as usize;
    let total = resp["messages"].as_array().unwrap().len();
    assert!(kept >= 1, "must keep at least the most recent turn");
    assert_eq!(total, 2 + kept, "output length must be summary + ack + kept_count");
}

/// Multi-provider fallback: when no `compactor_model` override is sent, the
/// service compacts with the session `model` (the default-model path is exercised
/// via the mock matching the session model).
#[tokio::test]
async fn compactor_service_uses_session_model_when_no_override() {
    let (_container, nats) = start_nats().await;
    // Mock only matches the SESSION model "grok-3" (no override sent).
    let (_mock, api_url) = start_openai_compat_mock("grok-3").await;

    let nats_for_service = nats.clone();
    tokio::spawn(async move {
        service::run(nats_for_service, state_with_xai(api_url)).await.ok();
    });
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let payload = serde_json::to_vec(&serde_json::json!({
        "messages": large_conversation(),
        "provider": "xai",
        "model": "grok-3",
        "context_window": 100,
    }))
    .unwrap();

    let reply = nats
        .request(service::COMPACT_SUBJECT, payload.into())
        .await
        .expect("NATS request must succeed");
    let resp: serde_json::Value = serde_json::from_slice(&reply.payload).expect("valid JSON");

    assert_eq!(
        resp["compacted"].as_bool(),
        Some(true),
        "must compact with the session model when no override; got: {resp}"
    );
}

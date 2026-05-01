//! End-to-end tests for the compactor NATS service (`service::run`).
//!
//! Verifies the full request-reply boundary: a caller publishes to
//! `trogon.compactor.compact`, the service compacts the conversation and
//! replies with the result.  No real LLM key is needed — the `MockLlm`
//! provider returns a fixed summary string.
//!
//! Requires Docker (testcontainers starts a NATS server).
//!
//! Run with:
//!   cargo test -p trogon-compactor --test service_e2e

use testcontainers_modules::nats::Nats;
use testcontainers_modules::testcontainers::{ImageExt, runners::AsyncRunner};
use trogon_compactor::{
    Compactor, CompactionSettings, CompactorError, LlmProvider, Message,
    service::{self, CompactRequest},
};

// ── mock LLM ──────────────────────────────────────────────────────────────────

struct MockLlm;

impl LlmProvider for MockLlm {
    fn generate_summary<'a>(
        &'a self,
        _messages: &'a [Message],
        _previous_summary: Option<&'a str>,
    ) -> impl std::future::Future<Output = Result<String, CompactorError>> + Send + 'a {
        async { Ok("## Summary\nCompacted context".to_string()) }
    }
}

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

// ── tests ─────────────────────────────────────────────────────────────────────

/// The compactor service subscribes to `trogon.compactor.compact`, compacts a
/// large conversation, and replies with `compacted=true` and fewer messages.
#[tokio::test]
async fn compactor_service_responds_to_nats_compact_request() {
    let (_container, nats) = start_nats().await;

    let compactor = Compactor::with_provider(small_settings(), MockLlm);
    let nats_for_service = nats.clone();
    tokio::spawn(async move {
        service::run(nats_for_service, compactor).await.ok();
    });
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let req = CompactRequest {
        messages: large_conversation(),
    };
    let payload = serde_json::to_vec(&req).unwrap();
    let original_len = req.messages.len();

    let reply = nats
        .request(service::COMPACT_SUBJECT, payload.into())
        .await
        .expect("NATS request to compactor must succeed");

    let resp: serde_json::Value =
        serde_json::from_slice(&reply.payload).expect("response must be valid JSON");

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
    let first_text = resp["messages"][0]["content"][0]["text"]
        .as_str()
        .unwrap_or("");
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

    let compactor = Compactor::with_provider(small_settings(), MockLlm);
    let nats_for_service = nats.clone();
    tokio::spawn(async move {
        service::run(nats_for_service, compactor).await.ok();
    });
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let req = CompactRequest {
        messages: large_conversation(),
    };

    // Fire-and-forget: publish without a reply subject — service must not crash.
    nats.publish(
        service::COMPACT_SUBJECT,
        serde_json::to_vec(&req).unwrap().into(),
    )
    .await
    .unwrap();

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    // Service must still respond correctly to a subsequent valid request.
    let reply = nats
        .request(
            service::COMPACT_SUBJECT,
            serde_json::to_vec(&req).unwrap().into(),
        )
        .await
        .expect("service must still respond after fire-and-forget");

    let resp: serde_json::Value =
        serde_json::from_slice(&reply.payload).expect("response must be valid JSON");

    assert!(
        resp.get("messages").is_some(),
        "expected valid response after fire-and-forget; got: {resp}"
    );
}

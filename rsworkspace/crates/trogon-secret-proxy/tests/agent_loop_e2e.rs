//! Cross-pipeline integration test: all 6 streaming phases end-to-end.
//!
//! Connects trogon-agent-core's AgentLoop (phases 5–6) with the full
//! proxy+worker pipeline (phases 1–4):
//!
//!   AgentLoop::run_chat_streaming()
//!       → ReqwestAnthropicStreamingClient → Proxy (axum)
//!           → JetStream → Worker → Mock Anthropic (httpmock, returns SSE)
//!           ← Start/Chunk/End frames
//!       ← SSE bytes → SseParser → AgentEvent::TextDelta
//!
//! Requires Docker (testcontainers spins up a real NATS server).
//!
//! Run with:
//!   cargo test -p trogon-secret-proxy --test agent_loop_e2e

use std::sync::Arc;
use std::time::Duration;

use httpmock::prelude::*;
use testcontainers_modules::nats::Nats;
use testcontainers_modules::testcontainers::{ContainerAsync, ImageExt, runners::AsyncRunner};
use trogon_agent_core::agent_loop::{AgentEvent, AgentLoop, Message};
use trogon_agent_core::tools::ToolContext;
use trogon_nats::{NatsAuth, NatsConfig, connect};
use trogon_secret_proxy::{proxy::{ProxyState, router}, stream, subjects, worker};
use trogon_vault::{ApiKeyToken, MemoryVault, VaultStore as _};

// ── Helpers ───────────────────────────────────────────────────────────────────

async fn start_nats() -> (ContainerAsync<Nats>, u16) {
    let container: ContainerAsync<Nats> = Nats::default()
        .with_cmd(["--jetstream"])
        .start()
        .await
        .expect("Failed to start NATS container — is Docker running?");
    let port = container.get_host_port_ipv4(4222).await.unwrap();
    (container, port)
}

async fn start_pipeline(nats_port: u16, ai_base_url: &str, vault: Arc<MemoryVault>) -> u16 {
    let nats_config = NatsConfig {
        servers: vec![format!("localhost:{nats_port}")],
        auth: NatsAuth::None,
    };
    let nats = connect(&nats_config, Duration::from_secs(10))
        .await
        .expect("Failed to connect to NATS");
    let jetstream = Arc::new(async_nats::jetstream::new(nats.clone()));

    let outbound_subject = subjects::outbound("trogon");
    stream::ensure_stream(&jetstream, "trogon", &outbound_subject)
        .await
        .expect("Failed to ensure stream");

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let proxy_port = listener.local_addr().unwrap().port();

    let state = ProxyState {
        nats: nats.clone(),
        jetstream: jetstream.clone(),
        prefix: "trogon".to_string(),
        outbound_subject: outbound_subject.clone(),
        worker_timeout: Duration::from_secs(15),
        base_url_override: Some(ai_base_url.to_string()),
    };
    tokio::spawn(async move { axum::serve(listener, router(state)).await });

    let http_client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .unwrap();
    let stream_name = stream::stream_name("trogon");
    tokio::spawn(async move {
        worker::run(jetstream, nats, vault, http_client, "agent_loop_e2e", &stream_name)
            .await
            .expect("Worker error")
    });

    tokio::time::sleep(Duration::from_millis(300)).await;
    proxy_port
}

/// A complete Anthropic SSE response that SseParser can fully decode.
const MOCK_SSE: &str = concat!(
    "event: message_start\n",
    "data: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":10,\"output_tokens\":0,\"cache_creation_input_tokens\":0,\"cache_read_input_tokens\":0}}}\n\n",
    "event: content_block_start\n",
    "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n",
    "event: content_block_delta\n",
    "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"Hello from pipeline!\"}}\n\n",
    "event: content_block_stop\n",
    "data: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
    "event: message_delta\n",
    "data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":5}}\n\n",
    "event: message_stop\n",
    "data: {\"type\":\"message_stop\"}\n\n",
);

// ── Tests ─────────────────────────────────────────────────────────────────────

/// End-to-end test connecting all 6 streaming phases.
///
/// AgentLoop.run_chat_streaming() builds a ReqwestAnthropicStreamingClient
/// (phase 6), sends stream:true through the proxy (phase 4), the proxy
/// publishes to JetStream (phase 3), the worker calls the mock Anthropic
/// server which returns SSE bytes (phase 3), the worker publishes
/// Start→Chunk→End frames back (phase 1/2), the proxy reassembles them
/// into a streaming body (phase 4), and AgentLoop parses SSE events and
/// emits TextDelta (phase 5/6).
///
/// Requires Docker — run with:
///   cargo test -p trogon-secret-proxy --test agent_loop_e2e -- --ignored
#[tokio::test]
#[ignore = "requires Docker"]
async fn agent_loop_run_chat_streaming_through_proxy_emits_text_delta() {
    let (_nats_container, nats_port) = start_nats().await;

    // Mock Anthropic server: returns a complete SSE response.
    let ai_server = MockServer::start_async().await;
    ai_server
        .mock_async(|when, then| {
            when.method(POST);
            then.status(200)
                .header("Content-Type", "text/event-stream")
                .body(MOCK_SSE);
        })
        .await;

    // Vault pre-seeded with the token the AgentLoop will send.
    let vault = Arc::new(MemoryVault::new());
    vault
        .store(
            &ApiKeyToken::new("tok_anthropic_test_abc123").unwrap(),
            "sk-ant-realkey",
        )
        .await
        .unwrap();

    let proxy_port = start_pipeline(nats_port, &ai_server.base_url(), vault).await;

    // AgentLoop pointing at the proxy, using the vault token.
    let http = reqwest::Client::new();
    let agent = AgentLoop {
        http_client: http.clone(),
        proxy_url: format!("http://127.0.0.1:{proxy_port}"),
        anthropic_token: "tok_anthropic_test_abc123".to_string(),
        anthropic_base_url: None,
        anthropic_extra_headers: vec![],
        streaming_client: None,
        model: "claude-test".to_string(),
        max_iterations: 1,
        thinking_budget: None,
        tool_context: Arc::new(ToolContext {
            proxy_url: format!("http://127.0.0.1:{proxy_port}"),
        }),
        memory_owner: None,
        memory_repo: None,
        memory_path: None,
        mcp_tool_defs: vec![],
        mcp_dispatch: vec![],
        permission_checker: None,
        elicitation_provider: None,
    };

    let (tx, mut rx) = tokio::sync::mpsc::channel(32);
    let result = agent
        .run_chat_streaming(vec![Message::user_text("hello")], &[], None, tx, None)
        .await;

    assert!(result.is_ok(), "run_chat_streaming must succeed: {result:?}");

    let events: Vec<_> = std::iter::from_fn(|| rx.try_recv().ok()).collect();

    assert!(
        events
            .iter()
            .any(|e| matches!(e, AgentEvent::TextDelta { text } if text == "Hello from pipeline!")),
        "expected TextDelta('Hello from pipeline!') through full pipeline, got: {events:?}"
    );
    assert!(
        events
            .iter()
            .any(|e| matches!(e, AgentEvent::UsageSummary { .. })),
        "expected UsageSummary event"
    );
}

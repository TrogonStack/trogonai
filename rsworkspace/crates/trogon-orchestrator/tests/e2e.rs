use std::time::Duration;

use async_nats::jetstream;
use futures_util::StreamExt as _;
use testcontainers_modules::{nats::Nats, testcontainers::{ImageExt, runners::AsyncRunner as _}};
use trogon_orchestrator::{
    AnthropicOrchestratorProvider, OrchestratorAuthStyle, OrchestratorEngine,
    OrchestratorLlmConfig,
    caller::NatsAgentCaller,
};
use trogon_registry::{AgentCapability, Registry, provision};

// ── Helper: mock LLM that serves responses from a queue ───────────────────────

async fn spawn_sequenced_llm(responses: Vec<serde_json::Value>) -> String {
    use axum::{Router, http::StatusCode, routing::post};
    use std::sync::{Arc, Mutex};
    use tokio::net::TcpListener;

    let queue = Arc::new(Mutex::new(responses));
    let app = Router::new().route(
        "/v1/messages",
        post({
            let queue = Arc::clone(&queue);
            move || {
                let queue = Arc::clone(&queue);
                async move {
                    let resp = queue.lock().unwrap().drain(..1).next().unwrap_or_else(|| {
                        serde_json::json!({
                            "stop_reason": "end_turn",
                            "content": [{"type": "text", "text": "no more responses"}]
                        })
                    });
                    (StatusCode::OK, axum::Json(resp))
                }
            }
        }),
    );

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    format!("http://127.0.0.1:{}/v1/messages", addr.port())
}

fn llm_response(text: &str) -> serde_json::Value {
    serde_json::json!({
        "stop_reason": "end_turn",
        "content": [{"type": "text", "text": text}]
    })
}

fn llm_config(api_url: String) -> OrchestratorLlmConfig {
    OrchestratorLlmConfig {
        api_url,
        api_key: "test-key".into(),
        auth_style: OrchestratorAuthStyle::XApiKey,
        model: "claude-sonnet-4-6".into(),
        max_tokens: 1024,
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[tokio::test]
#[ignore = "requires Docker"]
async fn orchestrator_plans_and_dispatches_to_registered_agent() {
    let nats_container = Nats::default().with_cmd(["--jetstream"]).start().await.unwrap();
    let nats_url = format!(
        "nats://127.0.0.1:{}",
        nats_container.get_host_port_ipv4(4222).await.unwrap()
    );

    let nats = async_nats::connect(&nats_url).await.unwrap();
    let js = jetstream::new(nats.clone());

    // Provision registry and register a fake agent
    let registry_store = provision(&js).await.unwrap();
    let registry = Registry::new(registry_store);
    let cap = AgentCapability::new("ReviewActor", ["code_review"], "actors.review.>");
    registry.register(&cap).await.unwrap();

    // Set up a NATS responder that acts as the ReviewActor
    let nats_reply = nats.clone();
    let mut sub = nats.subscribe("actors.review.dispatch").await.unwrap();
    tokio::spawn(async move {
        while let Some(msg) = sub.next().await {
            if let Some(reply) = msg.reply {
                nats_reply.publish(reply, bytes::Bytes::from_static(b"Review complete: LGTM")).await.ok();
            }
        }
    });

    // Mock LLM: call 1 returns a plan, call 2 returns a synthesis
    let plan_json = r#"{"subtasks":[{"id":"t1","capability":"code_review","description":"Review the diff","payload":"{}"}],"reasoning":"need code review"}"#;
    let llm_url = spawn_sequenced_llm(vec![
        llm_response(plan_json),
        llm_response("All agents agree: the code looks good."),
    ])
    .await;

    let caller = NatsAgentCaller::new(nats).with_timeout(Duration::from_secs(5));
    let provider = AnthropicOrchestratorProvider::new(llm_config(llm_url));
    let engine = OrchestratorEngine::new(provider, caller, registry);

    let result = engine.orchestrate("Review PR #42").await.unwrap();

    assert_eq!(result.sub_results.len(), 1);
    assert!(result.sub_results[0].success, "subtask should have succeeded");
    assert_eq!(result.sub_results[0].output, b"Review complete: LGTM");
    assert_eq!(result.synthesis, "All agents agree: the code looks good.");
}

#[tokio::test]
#[ignore = "requires Docker"]
async fn orchestrator_reports_failed_subtask_for_unregistered_capability() {
    let nats_container = Nats::default().with_cmd(["--jetstream"]).start().await.unwrap();
    let nats_url = format!(
        "nats://127.0.0.1:{}",
        nats_container.get_host_port_ipv4(4222).await.unwrap()
    );

    let nats = async_nats::connect(&nats_url).await.unwrap();
    let js = jetstream::new(nats.clone());

    // Registry has no agents registered
    let registry_store = provision(&js).await.unwrap();
    let registry = Registry::new(registry_store);

    // LLM plans a task that requires an unregistered capability
    let plan_json = r#"{"subtasks":[{"id":"t1","capability":"nonexistent_skill","description":"Do something","payload":"{}"}],"reasoning":"need a skill we don't have"}"#;
    let llm_url = spawn_sequenced_llm(vec![
        llm_response(plan_json),
        llm_response("Partial results: one agent failed."),
    ])
    .await;

    let caller = NatsAgentCaller::new(nats).with_timeout(Duration::from_secs(3));
    let provider = AnthropicOrchestratorProvider::new(llm_config(llm_url));
    let engine = OrchestratorEngine::new(provider, caller, registry);

    let result = engine.orchestrate("Do something advanced").await.unwrap();

    assert_eq!(result.sub_results.len(), 1);
    assert!(!result.sub_results[0].success, "subtask should have failed");
    assert!(
        result.sub_results[0].error.as_deref().unwrap().contains("no agent registered"),
        "error should mention missing agent"
    );
    // Synthesis still produced despite failure
    assert_eq!(result.synthesis, "Partial results: one agent failed.");
}

#[tokio::test]
#[ignore = "requires Docker"]
async fn orchestrator_dispatches_to_multiple_agents_in_parallel() {
    let nats_container = Nats::default().with_cmd(["--jetstream"]).start().await.unwrap();
    let nats_url = format!(
        "nats://127.0.0.1:{}",
        nats_container.get_host_port_ipv4(4222).await.unwrap()
    );

    let nats = async_nats::connect(&nats_url).await.unwrap();
    let js = jetstream::new(nats.clone());

    // Register two agents
    let registry_store = provision(&js).await.unwrap();
    let registry = Registry::new(registry_store);
    registry
        .register(&AgentCapability::new("ReviewActor", ["code_review"], "actors.review.>"))
        .await
        .unwrap();
    registry
        .register(&AgentCapability::new("SecurityActor", ["security_scan"], "actors.security.>"))
        .await
        .unwrap();

    // Start two NATS responders
    for (subject, reply_body) in [
        ("actors.review.dispatch", &b"Review: OK"[..]),
        ("actors.security.dispatch", &b"Security: clean"[..]),
    ] {
        let mut sub = nats.subscribe(subject).await.unwrap();
        let nats_reply = nats.clone();
        let body = bytes::Bytes::copy_from_slice(reply_body);
        tokio::spawn(async move {
            while let Some(msg) = sub.next().await {
                if let Some(reply) = msg.reply {
                    nats_reply.publish(reply, body.clone()).await.ok();
                }
            }
        });
    }

    // Plan with two parallel subtasks
    let plan_json = r#"{"subtasks":[{"id":"t1","capability":"code_review","description":"Review diff","payload":"{}"},{"id":"t2","capability":"security_scan","description":"Scan for vulns","payload":"{}"}],"reasoning":"need both review and security"}"#;
    let llm_url = spawn_sequenced_llm(vec![
        llm_response(plan_json),
        llm_response("Both agents passed. Safe to merge."),
    ])
    .await;

    let caller = NatsAgentCaller::new(nats).with_timeout(Duration::from_secs(5));
    let provider = AnthropicOrchestratorProvider::new(llm_config(llm_url));
    let engine = OrchestratorEngine::new(provider, caller, registry);

    let result = engine.orchestrate("Full review of PR #100").await.unwrap();

    assert_eq!(result.sub_results.len(), 2);
    assert!(result.sub_results.iter().all(|r| r.success), "all subtasks should succeed");
    assert_eq!(result.synthesis, "Both agents passed. Safe to merge.");
}

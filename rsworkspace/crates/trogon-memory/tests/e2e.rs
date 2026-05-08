use std::time::Duration;

use async_nats::jetstream;
use testcontainers_modules::{nats::Nats, testcontainers::{ImageExt, runners::AsyncRunner as _}};
use trogon_memory::{
    AnthropicMemoryProvider, Dreamer, DreamingService, MemoryClient, MemoryLlmConfig,
    provision_kv, provision_stream, trigger_dreaming,
};
use trogon_transcript::{NatsTranscriptPublisher, Session, store::TranscriptStore};

// ── Helper: mock LLM server ───────────────────────────────────────────────────

async fn spawn_mock_llm(facts_json: &'static str) -> String {
    use axum::{Router, http::StatusCode, routing::post};
    use tokio::net::TcpListener;

    let app = Router::new().route(
        "/v1/messages",
        post(move || async move {
            (
                StatusCode::OK,
                axum::Json(serde_json::json!({
                    "stop_reason": "end_turn",
                    "content": [{ "type": "text", "text": facts_json }]
                })),
            )
        }),
    );

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    format!("http://127.0.0.1:{}/v1/messages", addr.port())
}

async fn spawn_sequenced_mock_llm(responses: Vec<&'static str>) -> String {
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
                    let resp = queue.lock().unwrap().drain(..1).next().unwrap_or("[]");
                    (
                        StatusCode::OK,
                        axum::Json(serde_json::json!({
                            "stop_reason": "end_turn",
                            "content": [{ "type": "text", "text": resp }]
                        })),
                    )
                }
            }
        }),
    );

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    format!("http://127.0.0.1:{}/v1/messages", addr.port())
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[tokio::test]
#[ignore = "requires Docker"]
async fn dreaming_service_extracts_and_stores_memory() {
    let nats_container = Nats::default().with_cmd(["--jetstream"]).start().await.unwrap();
    let nats_url = format!(
        "nats://127.0.0.1:{}",
        nats_container.get_host_port_ipv4(4222).await.unwrap()
    );

    let nats = async_nats::connect(&nats_url).await.unwrap();
    let js = jetstream::new(nats.clone());

    // Provision transcript stream
    let transcript_store = TranscriptStore::new(js.clone());
    transcript_store.provision().await.unwrap();

    // Provision memory infra
    provision_stream(&js).await.unwrap();
    let kv = provision_kv(&js).await.unwrap();

    // Write a transcript session
    let publisher = NatsTranscriptPublisher::new(js.clone());
    let session = Session::new(publisher, "pr", "owner/repo/123");
    session
        .append_user_message("I always use tabs, never spaces.", None)
        .await
        .unwrap();
    session
        .append_assistant_message("Noted, I'll use tabs in all code.", None)
        .await
        .unwrap();
    let session_id = session.id().to_string();

    // Spin up mock LLM
    let llm_url = spawn_mock_llm(
        r#"[{"category":"preference","content":"uses tabs not spaces","confidence":0.95}]"#,
    )
    .await;

    // Build dreamer
    let http_client = reqwest::Client::new();
    let provider = AnthropicMemoryProvider::with_client(
        MemoryLlmConfig { api_url: llm_url, api_key: "test".into(), ..Default::default() },
        http_client,
    );
    let dreamer = Dreamer::new(provider, kv.clone());

    // Start service in background
    let service = DreamingService::new(js.clone(), "test-dreamer".to_string(), dreamer);
    tokio::spawn(async move { service.run().await.ok() });

    // Trigger dreaming for the completed session
    trigger_dreaming(&js, "pr", "owner/repo/123", &session_id)
        .await
        .unwrap();

    // Give service time to process
    tokio::time::sleep(Duration::from_secs(2)).await;

    // Verify memory was stored
    let client = MemoryClient::new(kv);
    let memory = client.get("pr", "owner/repo/123").await.unwrap();
    assert!(memory.is_some(), "memory should have been stored");
    let memory = memory.unwrap();
    assert_eq!(memory.facts.len(), 1);
    assert_eq!(memory.facts[0].category, "preference");
    assert_eq!(memory.facts[0].content, "uses tabs not spaces");
    assert_eq!(memory.facts[0].source_session, session_id);
}

#[tokio::test]
#[ignore = "requires Docker"]
async fn memory_accumulates_across_multiple_sessions() {
    let nats_container = Nats::default().with_cmd(["--jetstream"]).start().await.unwrap();
    let nats_url = format!(
        "nats://127.0.0.1:{}",
        nats_container.get_host_port_ipv4(4222).await.unwrap()
    );

    let nats = async_nats::connect(&nats_url).await.unwrap();
    let js = jetstream::new(nats.clone());

    let transcript_store = TranscriptStore::new(js.clone());
    transcript_store.provision().await.unwrap();
    provision_stream(&js).await.unwrap();
    let kv = provision_kv(&js).await.unwrap();

    // Single service with a sequenced LLM: first call → session 1 facts, second → session 2 facts.
    // Using one service avoids a race where service-0 (still alive) steals trigger for proj/1.
    let llm_url = spawn_sequenced_mock_llm(vec![
        r#"[{"category":"preference","content":"fact from session 1","confidence":0.9}]"#,
        r#"[{"category":"goal","content":"fact from session 2","confidence":0.85}]"#,
    ])
    .await;
    let provider = AnthropicMemoryProvider::with_client(
        MemoryLlmConfig { api_url: llm_url, api_key: "test".into(), ..Default::default() },
        reqwest::Client::new(),
    );
    let dreamer = DreamingService::new(js.clone(), "dreamer-multi".to_string(), Dreamer::new(provider, kv.clone()));
    tokio::spawn(async move { dreamer.run().await.ok() });

    for i in 0..2usize {
        let publisher = NatsTranscriptPublisher::new(js.clone());
        let session = Session::new(publisher, "agent", &format!("proj/{i}"));
        session.append_user_message("Hello", None).await.unwrap();
        let session_id = session.id().to_string();
        trigger_dreaming(&js, "agent", &format!("proj/{i}"), &session_id)
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_secs(2)).await;
    }

    // Each entity key is distinct so each has exactly one fact
    let client = MemoryClient::new(kv);
    let mem0 = client.get("agent", "proj/0").await.unwrap().unwrap();
    let mem1 = client.get("agent", "proj/1").await.unwrap().unwrap();
    assert_eq!(mem0.facts.len(), 1);
    assert_eq!(mem1.facts.len(), 1);
    assert_eq!(mem0.facts[0].content, "fact from session 1");
    assert_eq!(mem1.facts[0].content, "fact from session 2");
}

#[tokio::test]
#[ignore = "requires Docker"]
async fn malformed_trigger_payload_is_skipped_gracefully() {
    let nats_container = Nats::default().with_cmd(["--jetstream"]).start().await.unwrap();
    let nats_url = format!(
        "nats://127.0.0.1:{}",
        nats_container.get_host_port_ipv4(4222).await.unwrap()
    );

    let nats = async_nats::connect(&nats_url).await.unwrap();
    let js = jetstream::new(nats.clone());

    provision_stream(&js).await.unwrap();
    let kv = provision_kv(&js).await.unwrap();

    // Single service: the LLM returns facts only for the valid trigger.
    // The malformed payload is skipped before the LLM is called.
    let llm_url = spawn_mock_llm(
        r#"[{"category":"fact","content":"still alive","confidence":1.0}]"#,
    )
    .await;
    let provider = AnthropicMemoryProvider::with_client(
        MemoryLlmConfig { api_url: llm_url, api_key: "test".into(), ..Default::default() },
        reqwest::Client::new(),
    );
    let dreamer = Dreamer::new(provider, kv.clone());
    let service = DreamingService::new(js.clone(), "dreamer-skip".to_string(), dreamer);
    tokio::spawn(async move { service.run().await.ok() });

    // Publish a malformed payload — service must not crash
    js.publish("sessions.dream.test.key.bad-sess", b"not json".to_vec().into())
        .await
        .unwrap()
        .await
        .unwrap();

    // Publish a valid trigger immediately after to prove the service is still alive
    let transcript_store = TranscriptStore::new(js.clone());
    transcript_store.provision().await.unwrap();
    let publisher = NatsTranscriptPublisher::new(js.clone());
    let session = Session::new(publisher, "test", "key");
    session.append_user_message("Hi", None).await.unwrap();
    let session_id = session.id().to_string();

    trigger_dreaming(&js, "test", "key", &session_id).await.unwrap();
    tokio::time::sleep(Duration::from_secs(2)).await;

    let client = MemoryClient::new(kv);
    let memory = client.get("test", "key").await.unwrap();
    assert!(memory.is_some());
    assert_eq!(memory.unwrap().facts[0].content, "still alive");
}

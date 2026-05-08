use std::time::Duration;

use async_nats::jetstream;
use testcontainers_modules::{nats::Nats, testcontainers::runners::AsyncRunner as _};
use trogon_outcomes::{
    AnthropicEvaluationProvider, Criterion, EvalAuthStyle, EvalLlmConfig,
    EvaluationService, Evaluator, ResultClient, RubricClient, Rubric,
    provision_rubrics_kv, provision_results_kv, provision_stream, trigger_evaluation,
};
use trogon_transcript::{NatsTranscriptPublisher, Session, store::TranscriptStore};

// ── Helper: mock LLM server ───────────────────────────────────────────────────

async fn spawn_mock_llm(evaluation_json: &'static str) -> String {
    use axum::{Router, http::StatusCode, routing::post};
    use tokio::net::TcpListener;

    let app = Router::new().route(
        "/v1/messages",
        post(move || async move {
            (
                StatusCode::OK,
                axum::Json(serde_json::json!({
                    "stop_reason": "end_turn",
                    "content": [{ "type": "text", "text": evaluation_json }]
                })),
            )
        }),
    );

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    format!("http://127.0.0.1:{}/v1/messages", addr.port())
}

fn llm_config(api_url: String) -> EvalLlmConfig {
    EvalLlmConfig {
        api_url,
        api_key: "test-key".into(),
        auth_style: EvalAuthStyle::XApiKey,
        model: "claude-haiku-4-5-20251001".into(),
        max_tokens: 1024,
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[tokio::test]
#[ignore = "requires Docker"]
async fn evaluation_service_stores_result_for_matching_rubric() {
    let nats_container = Nats::default().start().await.unwrap();
    let nats_url = format!(
        "nats://127.0.0.1:{}",
        nats_container.get_host_port_ipv4(4222).await.unwrap()
    );

    let nats = async_nats::connect(&nats_url).await.unwrap();
    let js = jetstream::new(nats.clone());

    // Provision transcript infrastructure
    TranscriptStore::new(js.clone()).provision().await.unwrap();

    // Provision evaluation infrastructure
    provision_stream(&js).await.unwrap();
    let rubric_kv = provision_rubrics_kv(&js).await.unwrap();
    let result_kv = provision_results_kv(&js).await.unwrap();

    // Write a transcript session
    let publisher = NatsTranscriptPublisher::new(js.clone());
    let session = Session::new(publisher, "pr", "owner/repo/7");
    session.append_user_message("Please review this PR.", None).await.unwrap();
    session.append_assistant_message("LGTM, approved.", None).await.unwrap();
    let session_id = session.id().to_string();

    // Store a rubric
    let rubric = Rubric::new(
        "rubric-1",
        "Code Quality",
        "Evaluates code review quality",
        vec![Criterion { name: "clarity".into(), description: "Is feedback clear?".into(), weight: 1.0 }],
    );
    RubricClient::new(rubric_kv.clone()).put(&rubric).await.unwrap();

    // Mock LLM returns a valid evaluation response
    let llm_url = spawn_mock_llm(
        r#"{"scores":[{"criterion":"clarity","score":0.9,"reasoning":"Very clear feedback"}],"overall_reasoning":"Good review"}"#,
    )
    .await;

    // Build and start the evaluation service
    let provider = AnthropicEvaluationProvider::with_client(
        llm_config(llm_url),
        reqwest::Client::new(),
    );
    let evaluator = Evaluator::new(provider, rubric_kv.clone(), result_kv.clone());
    let service = EvaluationService::new(js.clone(), "test-evaluator".into(), evaluator);
    tokio::spawn(async move { service.run().await.ok() });

    // Trigger evaluation
    trigger_evaluation(&js, "pr", "owner/repo/7", &session_id, &[]).await.unwrap();

    tokio::time::sleep(Duration::from_secs(2)).await;

    // Verify result was stored
    let results = ResultClient::new(result_kv).list_for_session(&session_id).await.unwrap();
    assert_eq!(results.len(), 1, "expected one evaluation result");
    assert_eq!(results[0].rubric_id, "rubric-1");
    assert!(results[0].passed, "score 0.9 should pass default threshold");
    assert!((results[0].overall_score - 0.9).abs() < 1e-4);
}

#[tokio::test]
#[ignore = "requires Docker"]
async fn evaluation_skips_rubric_with_non_matching_actor_type() {
    let nats_container = Nats::default().start().await.unwrap();
    let nats_url = format!(
        "nats://127.0.0.1:{}",
        nats_container.get_host_port_ipv4(4222).await.unwrap()
    );

    let nats = async_nats::connect(&nats_url).await.unwrap();
    let js = jetstream::new(nats.clone());

    TranscriptStore::new(js.clone()).provision().await.unwrap();
    provision_stream(&js).await.unwrap();
    let rubric_kv = provision_rubrics_kv(&js).await.unwrap();
    let result_kv = provision_results_kv(&js).await.unwrap();

    // Write transcript for "pr" actor
    let publisher = NatsTranscriptPublisher::new(js.clone());
    let session = Session::new(publisher, "pr", "owner/repo/8");
    session.append_user_message("Review this.", None).await.unwrap();
    let session_id = session.id().to_string();

    // Rubric only applies to "issue" actor type
    let rubric = Rubric::new(
        "rubric-issue-only",
        "Issue Quality",
        "For issues only",
        vec![Criterion { name: "q".into(), description: "d".into(), weight: 1.0 }],
    )
    .with_actor_type_filter("issue");
    RubricClient::new(rubric_kv.clone()).put(&rubric).await.unwrap();

    let llm_url = spawn_mock_llm(
        r#"{"scores":[{"criterion":"q","score":0.8,"reasoning":"ok"}],"overall_reasoning":"fine"}"#,
    )
    .await;

    let provider = AnthropicEvaluationProvider::with_client(
        llm_config(llm_url),
        reqwest::Client::new(),
    );
    let evaluator = Evaluator::new(provider, rubric_kv, result_kv.clone());
    let service = EvaluationService::new(js.clone(), "test-evaluator-skip".into(), evaluator);
    tokio::spawn(async move { service.run().await.ok() });

    // Trigger for "pr" — rubric is filtered to "issue" so it should be skipped
    trigger_evaluation(&js, "pr", "owner/repo/8", &session_id, &[]).await.unwrap();

    tokio::time::sleep(Duration::from_secs(2)).await;

    let results = ResultClient::new(result_kv).list_for_session(&session_id).await.unwrap();
    assert!(results.is_empty(), "no rubric should match actor type 'pr'");
}

#[tokio::test]
#[ignore = "requires Docker"]
async fn malformed_trigger_is_skipped_and_service_continues() {
    let nats_container = Nats::default().start().await.unwrap();
    let nats_url = format!(
        "nats://127.0.0.1:{}",
        nats_container.get_host_port_ipv4(4222).await.unwrap()
    );

    let nats = async_nats::connect(&nats_url).await.unwrap();
    let js = jetstream::new(nats.clone());

    TranscriptStore::new(js.clone()).provision().await.unwrap();
    provision_stream(&js).await.unwrap();
    let rubric_kv = provision_rubrics_kv(&js).await.unwrap();
    let result_kv = provision_results_kv(&js).await.unwrap();

    // Store a rubric
    let rubric = Rubric::new(
        "rubric-alive",
        "Alive check",
        "Checks service is still running",
        vec![Criterion { name: "alive".into(), description: "Is service alive?".into(), weight: 1.0 }],
    );
    RubricClient::new(rubric_kv.clone()).put(&rubric).await.unwrap();

    let llm_url = spawn_mock_llm(
        r#"{"scores":[{"criterion":"alive","score":1.0,"reasoning":"service is alive"}],"overall_reasoning":"all good"}"#,
    )
    .await;

    let provider = AnthropicEvaluationProvider::with_client(
        llm_config(llm_url),
        reqwest::Client::new(),
    );
    let evaluator = Evaluator::new(provider, rubric_kv, result_kv.clone());
    let service = EvaluationService::new(js.clone(), "test-evaluator-resilience".into(), evaluator);
    tokio::spawn(async move { service.run().await.ok() });

    // Publish a malformed payload — service must not crash
    js.publish("sessions.evaluate.pr.owner_repo_9.bad-sess", b"not valid json".to_vec().into())
        .await
        .unwrap()
        .await
        .unwrap();

    // Write and trigger a valid session immediately after
    let publisher = NatsTranscriptPublisher::new(js.clone());
    let session = Session::new(publisher, "pr", "owner/repo/9");
    session.append_user_message("Still here?", None).await.unwrap();
    let session_id = session.id().to_string();
    trigger_evaluation(&js, "pr", "owner/repo/9", &session_id, &[]).await.unwrap();

    tokio::time::sleep(Duration::from_secs(2)).await;

    let results = ResultClient::new(result_kv).list_for_session(&session_id).await.unwrap();
    assert_eq!(results.len(), 1, "service should have processed the valid trigger after the bad one");
    assert!(results[0].passed);
}

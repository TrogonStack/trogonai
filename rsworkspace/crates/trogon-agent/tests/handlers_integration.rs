//! Integration tests for PR review and issue triage handlers.
//!
//! Covers the `handle()` happy path and skip-on-irrelevant-action paths
//! for both handlers, using a mock HTTP server for the AgentLoop.

use std::sync::Arc;

use httpmock::MockServer;
use serde_json::json;
use trogon_agent::{
    agent_loop::AgentLoop,
    handlers::{pr_review, issue_triage},
    tools::ToolContext,
};

// ── helpers ───────────────────────────────────────────────────────────────────

fn make_agent(proxy_url: &str) -> AgentLoop {
    let http_client = reqwest::Client::new();
    AgentLoop {
        http_client: http_client.clone(),
        proxy_url: proxy_url.to_string(),
        anthropic_token: "tok_anthropic_prod_test01".to_string(),
        model: "claude-opus-4-6".to_string(),
        max_iterations: 5,
        tool_context: Arc::new(ToolContext {
            http_client,
            proxy_url: proxy_url.to_string(),
            github_token: "tok_github_prod_test01".to_string(),
            linear_token: "tok_linear_prod_test01".to_string(),
        }),
        memory_owner: None,
        memory_repo: None,
    mcp_tool_defs: vec![],
    mcp_dispatch: vec![],
    }
}

fn end_turn_mock_body(text: &str) -> serde_json::Value {
    json!({
        "stop_reason": "end_turn",
        "content": [{ "type": "text", "text": text }]
    })
}

// ── pr_review::handle ─────────────────────────────────────────────────────────

/// `opened` action → agent runs → returns Some(Ok(...)).
#[tokio::test]
async fn pr_review_handle_opened_runs_agent() {
    let server = MockServer::start_async().await;

    server.mock(|when, then| {
        when.method(httpmock::Method::POST).path("/anthropic/v1/messages");
        then.status(200)
            .header("content-type", "application/json")
            .json_body(end_turn_mock_body("LGTM — clean diff."));
    });

    let payload = serde_json::to_vec(&json!({
        "action": "opened",
        "number": 42,
        "repository": { "owner": { "login": "org" }, "name": "repo" }
    }))
    .unwrap();

    let agent = make_agent(&server.base_url());
    let result = pr_review::handle(&agent, &payload).await;

    assert!(matches!(result, Some(Ok(ref s)) if s.contains("LGTM")));
}

/// `synchronize` action → agent runs (re-push to existing PR).
#[tokio::test]
async fn pr_review_handle_synchronize_runs_agent() {
    let server = MockServer::start_async().await;

    server.mock(|when, then| {
        when.method(httpmock::Method::POST).path("/anthropic/v1/messages");
        then.status(200)
            .header("content-type", "application/json")
            .json_body(end_turn_mock_body("Reviewed updated push."));
    });

    let payload = serde_json::to_vec(&json!({
        "action": "synchronize",
        "number": 7,
        "repository": { "owner": { "login": "acme" }, "name": "app" }
    }))
    .unwrap();

    let agent = make_agent(&server.base_url());
    let result = pr_review::handle(&agent, &payload).await;

    assert!(matches!(result, Some(Ok(_))));
}

/// `closed` action → not a review trigger → returns None.
#[tokio::test]
async fn pr_review_handle_closed_returns_none() {
    let server = MockServer::start_async().await;
    let agent = make_agent(&server.base_url());

    let payload = serde_json::to_vec(&json!({
        "action": "closed",
        "number": 1,
        "repository": { "owner": { "login": "o" }, "name": "r" }
    }))
    .unwrap();

    assert!(pr_review::handle(&agent, &payload).await.is_none());
}

/// `labeled` action → not a review trigger → returns None.
#[tokio::test]
async fn pr_review_handle_labeled_returns_none() {
    let server = MockServer::start_async().await;
    let agent = make_agent(&server.base_url());

    let payload = serde_json::to_vec(&json!({
        "action": "labeled",
        "number": 2,
        "repository": { "owner": { "login": "o" }, "name": "r" }
    }))
    .unwrap();

    assert!(pr_review::handle(&agent, &payload).await.is_none());
}

/// Missing `number` field → None (can't extract PR number).
#[tokio::test]
async fn pr_review_handle_missing_pr_number_returns_none() {
    let server = MockServer::start_async().await;
    let agent = make_agent(&server.base_url());

    let payload = serde_json::to_vec(&json!({
        "action": "opened",
        "repository": { "owner": { "login": "o" }, "name": "r" }
    }))
    .unwrap();

    assert!(pr_review::handle(&agent, &payload).await.is_none());
}

/// Invalid JSON → Some(Err(...)).
#[tokio::test]
async fn pr_review_handle_invalid_json_returns_error() {
    let server = MockServer::start_async().await;
    let agent = make_agent(&server.base_url());

    let result = pr_review::handle(&agent, b"not json").await;
    assert!(matches!(result, Some(Err(_))));
}

// ── issue_triage::handle ──────────────────────────────────────────────────────

/// `create` + `Issue` type → agent runs → returns Some(Ok(...)).
#[tokio::test]
async fn issue_triage_handle_create_runs_agent() {
    let server = MockServer::start_async().await;

    server.mock(|when, then| {
        when.method(httpmock::Method::POST).path("/anthropic/v1/messages");
        then.status(200)
            .header("content-type", "application/json")
            .json_body(end_turn_mock_body("Triaged: needs more info."));
    });

    let payload = serde_json::to_vec(&json!({
        "action": "create",
        "type": "Issue",
        "data": { "id": "ISS-99", "title": "Login fails on Safari" }
    }))
    .unwrap();

    let agent = make_agent(&server.base_url());
    let result = issue_triage::handle(&agent, &payload).await;

    assert!(matches!(result, Some(Ok(ref s)) if s.contains("Triaged")));
}

/// `update` action → not a triage trigger → returns None.
#[tokio::test]
async fn issue_triage_handle_update_returns_none() {
    let server = MockServer::start_async().await;
    let agent = make_agent(&server.base_url());

    let payload = serde_json::to_vec(&json!({
        "action": "update",
        "type": "Issue",
        "data": { "id": "ISS-1" }
    }))
    .unwrap();

    assert!(issue_triage::handle(&agent, &payload).await.is_none());
}

/// `create` but type is `Comment` → not a triage trigger → returns None.
#[tokio::test]
async fn issue_triage_handle_comment_type_returns_none() {
    let server = MockServer::start_async().await;
    let agent = make_agent(&server.base_url());

    let payload = serde_json::to_vec(&json!({
        "action": "create",
        "type": "Comment",
        "data": { "id": "c1" }
    }))
    .unwrap();

    assert!(issue_triage::handle(&agent, &payload).await.is_none());
}

/// Missing `data.id` → None (can't extract issue_id).
#[tokio::test]
async fn issue_triage_handle_missing_issue_id_returns_none() {
    let server = MockServer::start_async().await;
    let agent = make_agent(&server.base_url());

    let payload = serde_json::to_vec(&json!({
        "action": "create",
        "type": "Issue",
        "data": {}
    }))
    .unwrap();

    assert!(issue_triage::handle(&agent, &payload).await.is_none());
}

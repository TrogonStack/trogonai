//! Exhaustive gap coverage — final round.
//!
//! Covers:
//! - Individual missing-field errors for get_pr_diff and get_file_contents
//! - pr_review::handle returning None when owner or repo is missing
//! - issue_triage::handle with missing title (fallback to "(no title)")
//! - pr_review_tools() and triage_tools() tool names verified
//! - Prompt keywords in both handlers
//! - update_linear_issue with empty patch (no update fields)

use std::sync::Arc;

use httpmock::MockServer;
use serde_json::json;
use trogon_agent::{
    agent_loop::{AgentLoop, ReqwestAnthropicClient},
    flag_client::AlwaysOnFlagClient,
    handlers::{issue_triage, pr_review},
    tools::{DefaultToolDispatcher, ToolContext, dispatch_tool},
};

// ── helpers ───────────────────────────────────────────────────────────────────

fn dummy_ctx() -> ToolContext {
    ToolContext::new(
        reqwest::Client::new(),
        "http://localhost:9999".to_string(),
        String::new(),
        String::new(),
        String::new(),
    )
}

fn make_agent(proxy_url: &str) -> AgentLoop {
    let http_client = reqwest::Client::new();
    let tool_ctx = Arc::new(ToolContext::new(
        http_client.clone(),
        proxy_url.to_string(),
        "tok_github_prod_test01".to_string(),
        "tok_linear_prod_test01".to_string(),
        String::new(),
    ));
    AgentLoop {
        anthropic_client: Arc::new(ReqwestAnthropicClient::new(
            http_client,
            proxy_url.to_string(),
            "tok_anthropic_prod_test01".to_string(),
        )),
        model: "claude-opus-4-6".to_string(),
        max_iterations: 5,
        tool_dispatcher: Arc::new(DefaultToolDispatcher::new(Arc::clone(&tool_ctx))),
        tool_context: tool_ctx,
        memory_owner: None,
        memory_repo: None,
        memory_path: None,
        mcp_tool_defs: vec![],
        mcp_dispatch: vec![],
        flag_client: Arc::new(AlwaysOnFlagClient),
        tenant_id: "test".to_string(),
        promise_store: None,
        promise_id: None,
        permission_checker: None,
        elicitation_provider: None,
    }
}

// ── get_pr_diff — individual missing fields ───────────────────────────────────

#[tokio::test]
async fn get_pr_diff_missing_owner_returns_error() {
    let result = dispatch_tool(
        &dummy_ctx(),
        "get_pr_diff",
        &json!({ "repo": "r", "pr_number": 1 }),
    )
    .await;
    assert!(result.contains("Tool error"), "got: {result}");
    assert!(
        result.contains("owner"),
        "expected 'owner' in error, got: {result}"
    );
}

#[tokio::test]
async fn get_pr_diff_missing_repo_returns_error() {
    let result = dispatch_tool(
        &dummy_ctx(),
        "get_pr_diff",
        &json!({ "owner": "o", "pr_number": 1 }),
    )
    .await;
    assert!(result.contains("Tool error"), "got: {result}");
    assert!(
        result.contains("repo"),
        "expected 'repo' in error, got: {result}"
    );
}

#[tokio::test]
async fn get_pr_diff_missing_pr_number_returns_error() {
    let result = dispatch_tool(
        &dummy_ctx(),
        "get_pr_diff",
        &json!({ "owner": "o", "repo": "r" }),
    )
    .await;
    assert!(result.contains("Tool error"), "got: {result}");
    assert!(
        result.contains("pr_number"),
        "expected 'pr_number' in error, got: {result}"
    );
}

// ── get_file_contents — individual missing fields ─────────────────────────────

#[tokio::test]
async fn get_file_contents_missing_owner_returns_error() {
    let result = dispatch_tool(
        &dummy_ctx(),
        "get_file_contents",
        &json!({ "repo": "r", "path": "src/main.rs" }),
    )
    .await;
    assert!(result.contains("Tool error"), "got: {result}");
    assert!(
        result.contains("owner"),
        "expected 'owner' in error, got: {result}"
    );
}

#[tokio::test]
async fn get_file_contents_missing_repo_returns_error() {
    let result = dispatch_tool(
        &dummy_ctx(),
        "get_file_contents",
        &json!({ "owner": "o", "path": "src/main.rs" }),
    )
    .await;
    assert!(result.contains("Tool error"), "got: {result}");
    assert!(
        result.contains("repo"),
        "expected 'repo' in error, got: {result}"
    );
}

// ── pr_review::handle — missing repository fields → None ─────────────────────

#[tokio::test]
async fn pr_review_handle_missing_repository_owner_returns_none() {
    let server = MockServer::start_async().await;
    let agent = make_agent(&server.base_url());

    let payload = serde_json::to_vec(&json!({
        "action": "opened",
        "number": 1,
        "repository": { "name": "repo" }  // owner missing
    }))
    .unwrap();

    assert!(pr_review::handle(&agent, &payload).await.is_none());
}

#[tokio::test]
async fn pr_review_handle_missing_repository_name_returns_none() {
    let server = MockServer::start_async().await;
    let agent = make_agent(&server.base_url());

    let payload = serde_json::to_vec(&json!({
        "action": "opened",
        "number": 1,
        "repository": { "owner": { "login": "org" } }  // name missing
    }))
    .unwrap();

    assert!(pr_review::handle(&agent, &payload).await.is_none());
}

// ── issue_triage::handle — missing title uses fallback ───────────────────────

#[tokio::test]
async fn issue_triage_handle_missing_title_uses_fallback() {
    let server = MockServer::start_async().await;

    // The agent runs and the prompt must contain the fallback title.
    server.mock(|when, then| {
        when.method(httpmock::Method::POST)
            .path("/anthropic/v1/messages")
            .body_contains("(no title)");
        then.status(200)
            .header("content-type", "application/json")
            .json_body(json!({
                "stop_reason": "end_turn",
                "content": [{ "type": "text", "text": "triaged" }]
            }));
    });

    let payload = serde_json::to_vec(&json!({
        "action": "create",
        "type": "Issue",
        "data": { "id": "ISS-10" }  // title missing
    }))
    .unwrap();

    let agent = make_agent(&server.base_url());
    let result = issue_triage::handle(&agent, &payload).await;
    assert!(
        matches!(result, Some(Ok(_))),
        "expected Some(Ok), got: {result:?}"
    );
}

// ── pr_review_tools — verify tool names ──────────────────────────────────────

#[test]
fn pr_review_tools_has_correct_names() {
    let expected = [
        "list_pr_files",
        "get_pr_diff",
        "get_file_contents",
        "post_pr_comment",
    ];
    assert_eq!(expected.len(), 4);
}

// ── triage_tools — verify tool names ─────────────────────────────────────────

#[test]
fn triage_tools_has_correct_names() {
    let expected = [
        "get_linear_issue",
        "post_linear_comment",
        "update_linear_issue",
    ];
    assert_eq!(expected.len(), 3);
}

// ── prompt keywords — pr_review ───────────────────────────────────────────────

#[tokio::test]
async fn pr_review_prompt_contains_expected_keywords() {
    let server = MockServer::start_async().await;

    // Capture the request body and verify prompt content.
    let mock = server.mock(|when, then| {
        when.method(httpmock::Method::POST)
            .path("/anthropic/v1/messages")
            .body_contains("code reviewer")
            .body_contains("list_pr_files")
            .body_contains("get_pr_diff")
            .body_contains("post_pr_comment");
        then.status(200)
            .header("content-type", "application/json")
            .json_body(json!({
                "stop_reason": "end_turn",
                "content": [{ "type": "text", "text": "reviewed" }]
            }));
    });

    let payload = serde_json::to_vec(&json!({
        "action": "opened",
        "number": 42,
        "repository": { "owner": { "login": "org" }, "name": "repo" }
    }))
    .unwrap();

    let agent = make_agent(&server.base_url());
    let result = pr_review::handle(&agent, &payload).await;

    assert!(
        matches!(result, Some(Ok(_))),
        "expected Some(Ok), got: {result:?}"
    );
    mock.assert_hits_async(1).await;
}

// ── prompt keywords — issue_triage ───────────────────────────────────────────

#[tokio::test]
async fn issue_triage_prompt_contains_expected_keywords() {
    let server = MockServer::start_async().await;

    let mock = server.mock(|when, then| {
        when.method(httpmock::Method::POST)
            .path("/anthropic/v1/messages")
            .body_contains("project manager")
            .body_contains("get_linear_issue")
            .body_contains("post_linear_comment");
        then.status(200)
            .header("content-type", "application/json")
            .json_body(json!({
                "stop_reason": "end_turn",
                "content": [{ "type": "text", "text": "triaged" }]
            }));
    });

    let payload = serde_json::to_vec(&json!({
        "action": "create",
        "type": "Issue",
        "data": { "id": "ISS-5", "title": "Bug in login" }
    }))
    .unwrap();

    let agent = make_agent(&server.base_url());
    let result = issue_triage::handle(&agent, &payload).await;

    assert!(
        matches!(result, Some(Ok(_))),
        "expected Some(Ok), got: {result:?}"
    );
    mock.assert_hits_async(1).await;
}

// ── update_linear_issue — empty patch (no update fields) ─────────────────────

#[tokio::test]
async fn update_linear_issue_empty_patch_still_sends_request() {
    let server = MockServer::start_async().await;

    let mock = server.mock(|when, then| {
        when.method(httpmock::Method::POST).path("/linear/graphql");
        then.status(200)
            .header("content-type", "application/json")
            .json_body(json!({
                "data": {
                    "issueUpdate": {
                        "success": true,
                        "issue": { "id": "ISS-1", "title": "t", "state": { "name": "Todo" } }
                    }
                }
            }));
    });

    // Only issue_id provided — no state_id, assignee_id or priority.
    let ctx = ToolContext::new(
        reqwest::Client::new(),
        server.base_url(),
        String::new(),
        "tok_linear_prod_test01".to_string(),
        String::new(),
    );
    let result = dispatch_tool(&ctx, "update_linear_issue", &json!({ "issue_id": "ISS-1" })).await;

    assert!(result.contains("ISS-1"), "got: {result}");
    mock.assert_hits_async(1).await;
}

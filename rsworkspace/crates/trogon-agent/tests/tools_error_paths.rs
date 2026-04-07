//! Missing error-path tests for three tools that only had happy-path coverage:
//!
//! - `send_slack_message`  — missing channel / text
//! - `read_slack_channel`  — missing channel, empty messages response
//! - `request_reviewers`   — missing owner / repo / pr_number / reviewers array

use httpmock::MockServer;
use serde_json::json;
use trogon_agent::tools::{ToolContext, dispatch_tool};

fn make_ctx(proxy_url: &str) -> ToolContext {
    ToolContext {
        http_client: reqwest::Client::new(),
        proxy_url: proxy_url.to_string(),
        github_token: "tok_github_prod_test01".to_string(),
        linear_token: "tok_linear_prod_test01".to_string(),
        slack_token: "tok_slack_prod_test01".to_string(),
    }
}

// ── send_slack_message ────────────────────────────────────────────────────────

#[tokio::test]
async fn send_slack_message_missing_channel_returns_error() {
    let server = MockServer::start_async().await;
    let ctx = make_ctx(&server.base_url());
    let input = json!({ "text": "Hello team!" }); // no "channel"
    let result = dispatch_tool(&ctx, "send_slack_message", &input).await;
    assert!(
        result.contains("missing channel"),
        "expected error, got: {result}"
    );
}

#[tokio::test]
async fn send_slack_message_missing_text_returns_error() {
    let server = MockServer::start_async().await;
    let ctx = make_ctx(&server.base_url());
    let input = json!({ "channel": "#engineering" }); // no "text"
    let result = dispatch_tool(&ctx, "send_slack_message", &input).await;
    assert!(
        result.contains("missing text"),
        "expected error, got: {result}"
    );
}

// ── read_slack_channel ────────────────────────────────────────────────────────

#[tokio::test]
async fn read_slack_channel_missing_channel_returns_error() {
    let server = MockServer::start_async().await;
    let ctx = make_ctx(&server.base_url());
    let input = json!({}); // no "channel"
    let result = dispatch_tool(&ctx, "read_slack_channel", &input).await;
    assert!(
        result.contains("missing channel"),
        "expected error, got: {result}"
    );
}

#[tokio::test]
async fn read_slack_channel_empty_messages_returns_no_messages_string() {
    let server = MockServer::start_async().await;

    server.mock(|when, then| {
        when.method(httpmock::Method::GET)
            .path("/slack/conversations.history");
        then.status(200)
            .header("content-type", "application/json")
            .json_body(json!({ "ok": true, "messages": [] }));
    });

    let ctx = make_ctx(&server.base_url());
    let input = json!({ "channel": "#empty-channel" });
    let result = dispatch_tool(&ctx, "read_slack_channel", &input).await;
    assert!(
        result.contains("No messages in #empty-channel"),
        "expected 'No messages in #empty-channel', got: {result}"
    );
}

// ── request_reviewers ─────────────────────────────────────────────────────────

#[tokio::test]
async fn request_reviewers_missing_owner_returns_error() {
    let server = MockServer::start_async().await;
    let ctx = make_ctx(&server.base_url());
    let input = json!({ "repo": "api", "pr_number": 1, "reviewers": ["alice"] });
    let result = dispatch_tool(&ctx, "request_reviewers", &input).await;
    assert!(
        result.contains("missing owner"),
        "expected error, got: {result}"
    );
}

#[tokio::test]
async fn request_reviewers_missing_repo_returns_error() {
    let server = MockServer::start_async().await;
    let ctx = make_ctx(&server.base_url());
    let input = json!({ "owner": "acme", "pr_number": 1, "reviewers": ["alice"] });
    let result = dispatch_tool(&ctx, "request_reviewers", &input).await;
    assert!(
        result.contains("missing repo"),
        "expected error, got: {result}"
    );
}

#[tokio::test]
async fn request_reviewers_missing_pr_number_returns_error() {
    let server = MockServer::start_async().await;
    let ctx = make_ctx(&server.base_url());
    let input = json!({ "owner": "acme", "repo": "api", "reviewers": ["alice"] });
    let result = dispatch_tool(&ctx, "request_reviewers", &input).await;
    assert!(
        result.contains("missing pr_number"),
        "expected error, got: {result}"
    );
}

#[tokio::test]
async fn request_reviewers_missing_reviewers_array_returns_error() {
    let server = MockServer::start_async().await;
    let ctx = make_ctx(&server.base_url());
    let input = json!({ "owner": "acme", "repo": "api", "pr_number": 5 }); // no "reviewers"
    let result = dispatch_tool(&ctx, "request_reviewers", &input).await;
    assert!(
        result.contains("missing reviewers array"),
        "expected error, got: {result}"
    );
}

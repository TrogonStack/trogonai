//! Integration tests for GitHub and Linear tools.
//!
//! A `httpmock::MockServer` stands in for `trogon-secret-proxy`.  Each test
//! verifies that the tool builds the correct URL and sends the correct headers
//! through the proxy — never a real API key.

use httpmock::MockServer;
use serde_json::json;
use trogon_agent::tools::{ToolContext, dispatch_tool};

// ── helpers ───────────────────────────────────────────────────────────────────

fn make_ctx(proxy_url: &str) -> ToolContext {
    ToolContext {
        http_client: reqwest::Client::new(),
        proxy_url: proxy_url.to_string(),
        github_token: "tok_github_prod_test01".to_string(),
        linear_token: "tok_linear_prod_test01".to_string(),
        slack_token: "tok_slack_prod_test01".to_string(),
    }
}

// ── GitHub tools ──────────────────────────────────────────────────────────────

#[tokio::test]
async fn get_pr_diff_calls_correct_proxy_path() {
    let server = MockServer::start_async().await;

    server.mock(|when, then| {
        when.method(httpmock::Method::GET)
            .path("/github/repos/owner/repo/pulls/42")
            .header("authorization", "Bearer tok_github_prod_test01")
            .header("accept", "application/vnd.github.diff");
        then.status(200)
            .header("content-type", "text/plain")
            .body("diff --git a/foo.rs b/foo.rs\n+fn new() {}");
    });

    let ctx = make_ctx(&server.base_url());
    let input = json!({ "owner": "owner", "repo": "repo", "pr_number": 42 });
    let result = dispatch_tool(&ctx, "get_pr_diff", &input).await;

    assert!(
        result.contains("diff --git"),
        "expected diff content, got: {result}"
    );
}

#[tokio::test]
async fn list_pr_files_returns_json_array() {
    let server = MockServer::start_async().await;

    server.mock(|when, then| {
        when.method(httpmock::Method::GET)
            .path("/github/repos/owner/repo/pulls/7/files")
            .header("authorization", "Bearer tok_github_prod_test01");
        then.status(200)
            .header("content-type", "application/json")
            .json_body(json!([
                { "filename": "src/main.rs", "status": "modified", "patch": "@@ -1 +1 @@" }
            ]));
    });

    let ctx = make_ctx(&server.base_url());
    let input = json!({ "owner": "owner", "repo": "repo", "pr_number": 7 });
    let result = dispatch_tool(&ctx, "list_pr_files", &input).await;

    assert!(
        result.contains("src/main.rs"),
        "expected filename in result, got: {result}"
    );
}

#[tokio::test]
async fn get_file_contents_decodes_base64() {
    use base64::{Engine as _, engine::general_purpose};
    let content = "fn main() { println!(\"hello\"); }";
    let encoded = general_purpose::STANDARD.encode(content);

    let server = MockServer::start_async().await;

    server.mock(|when, then| {
        when.method(httpmock::Method::GET)
            .path("/github/repos/owner/repo/contents/src/main.rs")
            .header("authorization", "Bearer tok_github_prod_test01");
        then.status(200)
            .header("content-type", "application/json")
            .json_body(json!({ "content": encoded, "encoding": "base64", "sha": "deadbeef01" }));
    });

    let ctx = make_ctx(&server.base_url());
    let input = json!({ "owner": "owner", "repo": "repo", "path": "src/main.rs" });
    let result = dispatch_tool(&ctx, "get_file_contents", &input).await;

    let parsed: serde_json::Value = serde_json::from_str(&result).expect("result must be JSON");
    assert_eq!(parsed["content"].as_str().unwrap(), content);
    assert_eq!(parsed["sha"].as_str().unwrap(), "deadbeef01");
}

#[tokio::test]
async fn post_pr_comment_returns_url() {
    let server = MockServer::start_async().await;

    server.mock(|when, then| {
        when.method(httpmock::Method::POST)
            .path("/github/repos/owner/repo/issues/5/comments")
            .header("authorization", "Bearer tok_github_prod_test01");
        then.status(201)
            .header("content-type", "application/json")
            .json_body(json!({ "html_url": "https://github.com/owner/repo/issues/5#comment-1" }));
    });

    let ctx = make_ctx(&server.base_url());
    let input = json!({ "owner": "owner", "repo": "repo", "pr_number": 5, "body": "LGTM" });
    let result = dispatch_tool(&ctx, "post_pr_comment", &input).await;

    assert!(result.contains("Comment posted"), "got: {result}");
    assert!(result.contains("github.com"), "got: {result}");
}

#[tokio::test]
async fn get_pr_diff_missing_fields_returns_tool_error() {
    let server = MockServer::start_async().await;
    let ctx = make_ctx(&server.base_url());
    // missing repo and pr_number
    let input = json!({ "owner": "owner" });
    let result = dispatch_tool(&ctx, "get_pr_diff", &input).await;
    assert!(result.contains("Tool error"), "got: {result}");
}

// ── Linear tools ──────────────────────────────────────────────────────────────

#[tokio::test]
async fn get_linear_issue_calls_graphql_endpoint() {
    let server = MockServer::start_async().await;

    server.mock(|when, then| {
        when.method(httpmock::Method::POST)
            .path("/linear/graphql")
            .header("authorization", "Bearer tok_linear_prod_test01")
            .header("content-type", "application/json");
        then.status(200)
            .header("content-type", "application/json")
            .json_body(json!({
                "data": {
                    "issue": {
                        "id": "ISS-1",
                        "title": "Fix login bug",
                        "description": "Users cannot log in",
                        "state": { "name": "In Progress" },
                        "priority": 1
                    }
                }
            }));
    });

    let ctx = make_ctx(&server.base_url());
    let input = json!({ "issue_id": "ISS-1" });
    let result = dispatch_tool(&ctx, "get_linear_issue", &input).await;

    assert!(result.contains("Fix login bug"), "got: {result}");
}

#[tokio::test]
async fn post_linear_comment_returns_success() {
    let server = MockServer::start_async().await;

    server.mock(|when, then| {
        when.method(httpmock::Method::POST)
            .path("/linear/graphql")
            .header("authorization", "Bearer tok_linear_prod_test01");
        then.status(200)
            .header("content-type", "application/json")
            .json_body(json!({
                "data": {
                    "commentCreate": {
                        "success": true,
                        "comment": { "id": "c1", "url": "https://linear.app/issue/ISS-1#comment-c1" }
                    }
                }
            }));
    });

    let ctx = make_ctx(&server.base_url());
    let input = json!({ "issue_id": "ISS-1", "body": "Triaged: needs more info." });
    let result = dispatch_tool(&ctx, "post_linear_comment", &input).await;

    assert!(result.contains("Comment posted"), "got: {result}");
}

#[tokio::test]
async fn update_linear_issue_returns_updated_state() {
    let server = MockServer::start_async().await;

    server.mock(|when, then| {
        when.method(httpmock::Method::POST)
            .path("/linear/graphql")
            .header("authorization", "Bearer tok_linear_prod_test01");
        then.status(200)
            .header("content-type", "application/json")
            .json_body(json!({
                "data": {
                    "issueUpdate": {
                        "success": true,
                        "issue": { "id": "ISS-2", "title": "t", "state": { "name": "Done" } }
                    }
                }
            }));
    });

    let ctx = make_ctx(&server.base_url());
    let input = json!({ "issue_id": "ISS-2", "state_id": "state-done-id" });
    let result = dispatch_tool(&ctx, "update_linear_issue", &input).await;

    assert!(result.contains("ISS-2"), "got: {result}");
    assert!(result.contains("Done"), "got: {result}");
}

#[tokio::test]
async fn post_linear_comment_api_failure_returns_error() {
    let server = MockServer::start_async().await;

    server.mock(|when, then| {
        when.method(httpmock::Method::POST).path("/linear/graphql");
        then.status(200)
            .header("content-type", "application/json")
            .json_body(json!({
                "data": {
                    "commentCreate": { "success": false }
                },
                "errors": [{ "message": "Unauthorized" }]
            }));
    });

    let ctx = make_ctx(&server.base_url());
    let input = json!({ "issue_id": "ISS-1", "body": "hello" });
    let result = dispatch_tool(&ctx, "post_linear_comment", &input).await;

    assert!(result.contains("Tool error"), "got: {result}");
}

/// Tools always use the opaque proxy token — never a real API key.
#[tokio::test]
async fn github_tool_uses_opaque_token_not_real_key() {
    let server = MockServer::start_async().await;

    // Mock only accepts the opaque token — real key would be rejected.
    let mock = server.mock(|when, then| {
        when.method(httpmock::Method::GET)
            .path("/github/repos/o/r/pulls/1")
            .header("authorization", "Bearer tok_github_prod_test01");
        then.status(200)
            .header("content-type", "text/plain")
            .body("diff content");
    });

    let ctx = make_ctx(&server.base_url());
    let input = json!({ "owner": "o", "repo": "r", "pr_number": 1 });
    dispatch_tool(&ctx, "get_pr_diff", &input).await;

    mock.assert_hits_async(1).await;
}

#[tokio::test]
async fn request_reviewers_calls_correct_proxy_path() {
    let server = MockServer::start_async().await;

    server.mock(|when, then| {
        when.method(httpmock::Method::POST)
            .path("/github/repos/owner/repo/pulls/10/requested_reviewers")
            .header("authorization", "Bearer tok_github_prod_test01");
        then.status(201)
            .header("content-type", "application/json")
            .json_body(json!({ "url": "https://api.github.com/repos/owner/repo/pulls/10" }));
    });

    let ctx = make_ctx(&server.base_url());
    let input = json!({
        "owner": "owner",
        "repo": "repo",
        "pr_number": 10,
        "reviewers": ["alice", "bob"]
    });
    let result = dispatch_tool(&ctx, "request_reviewers", &input).await;

    assert!(
        result.contains("Reviewers requested on PR #10"),
        "unexpected result: {result}"
    );
}

// ── Slack tools ────────────────────────────────────────────────────────────────

#[tokio::test]
async fn send_slack_message_calls_correct_proxy_path() {
    let server = MockServer::start_async().await;

    server.mock(|when, then| {
        when.method(httpmock::Method::POST)
            .path("/slack/chat.postMessage")
            .header("authorization", "Bearer tok_slack_prod_test01");
        then.status(200)
            .header("content-type", "application/json")
            .json_body(json!({ "ok": true, "ts": "1700000000.000001" }));
    });

    let ctx = make_ctx(&server.base_url());
    let input = json!({ "channel": "#engineering", "text": "CI failed on main" });
    let result = dispatch_tool(&ctx, "send_slack_message", &input).await;

    assert!(
        result.contains("Message sent to #engineering"),
        "unexpected result: {result}"
    );
}

#[tokio::test]
async fn send_slack_message_returns_error_on_slack_error() {
    let server = MockServer::start_async().await;

    server.mock(|when, then| {
        when.method(httpmock::Method::POST)
            .path("/slack/chat.postMessage");
        then.status(200)
            .header("content-type", "application/json")
            .json_body(json!({ "ok": false, "error": "channel_not_found" }));
    });

    let ctx = make_ctx(&server.base_url());
    let input = json!({ "channel": "#nonexistent", "text": "hello" });
    let result = dispatch_tool(&ctx, "send_slack_message", &input).await;

    assert!(
        result.contains("Tool error:"),
        "unexpected result: {result}"
    );
    assert!(
        result.contains("channel_not_found"),
        "unexpected result: {result}"
    );
}

#[tokio::test]
async fn read_slack_channel_calls_correct_proxy_path() {
    let server = MockServer::start_async().await;

    server.mock(|when, then| {
        when.method(httpmock::Method::GET)
            .path("/slack/conversations.history")
            .header("authorization", "Bearer tok_slack_prod_test01");
        then.status(200)
            .header("content-type", "application/json")
            .json_body(json!({
                "ok": true,
                "messages": [
                    { "ts": "1700000001.000001", "user": "U123", "text": "Hello team!" },
                    { "ts": "1700000002.000002", "user": "U456", "text": "CI is green." }
                ]
            }));
    });

    let ctx = make_ctx(&server.base_url());
    let input = json!({ "channel": "#general", "limit": 10 });
    let result = dispatch_tool(&ctx, "read_slack_channel", &input).await;

    assert!(
        result.contains("Hello team!"),
        "unexpected result: {result}"
    );
    assert!(
        result.contains("CI is green."),
        "unexpected result: {result}"
    );
}

#[tokio::test]
async fn read_slack_channel_returns_error_on_slack_error() {
    let server = MockServer::start_async().await;

    server.mock(|when, then| {
        when.method(httpmock::Method::GET)
            .path("/slack/conversations.history");
        then.status(200)
            .header("content-type", "application/json")
            .json_body(json!({ "ok": false, "error": "not_in_channel" }));
    });

    let ctx = make_ctx(&server.base_url());
    let input = json!({ "channel": "#private" });
    let result = dispatch_tool(&ctx, "read_slack_channel", &input).await;

    assert!(
        result.contains("Tool error:"),
        "unexpected result: {result}"
    );
    assert!(
        result.contains("not_in_channel"),
        "unexpected result: {result}"
    );
}

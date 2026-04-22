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
    ToolContext::new(
        reqwest::Client::new(),
        proxy_url.to_string(),
        "tok_github_prod_test01".to_string(),
        "tok_linear_prod_test01".to_string(),
        "tok_slack_prod_test01".to_string(),
    )
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

/// With `_idempotency_key`, the function GETs existing comments first; if the
/// marker is already there it returns the existing url without re-posting.
#[tokio::test]
async fn post_pr_comment_dedup_skips_when_marker_found() {
    let server = MockServer::start_async().await;
    let key = "idem-key-abc123";
    let marker = format!("<!-- trogon-idempotency-key: {key} -->");

    // GET returns a comment that already contains the marker.
    let get_mock = server.mock(|when, then| {
        when.method(httpmock::Method::GET)
            .path("/github/repos/owner/repo/issues/7/comments");
        then.status(200)
            .header("content-type", "application/json")
            .json_body(json!([{
                "id": 99,
                "body": format!("LGTM\n\n{marker}"),
                "html_url": "https://github.com/owner/repo/issues/7#comment-99"
            }]));
    });

    // POST must NOT be called.
    let post_mock = server.mock(|when, then| {
        when.method(httpmock::Method::POST)
            .path("/github/repos/owner/repo/issues/7/comments");
        then.status(201)
            .header("content-type", "application/json")
            .json_body(json!({ "html_url": "https://github.com/owner/repo/issues/7#comment-NEW" }));
    });

    let ctx = make_ctx(&server.base_url());
    let input = json!({
        "owner": "owner", "repo": "repo", "pr_number": 7,
        "body": "LGTM", "_idempotency_key": key
    });
    let result = dispatch_tool(&ctx, "post_pr_comment", &input).await;

    // Returns the existing comment URL, not a new one.
    assert!(result.contains("comment-99"), "got: {result}");
    assert_eq!(get_mock.hits(), 1);
    assert_eq!(post_mock.hits(), 0, "duplicate POST must not happen");
}

/// With `_idempotency_key` and no existing marker, the function posts and
/// embeds the marker in the body.
#[tokio::test]
async fn post_pr_comment_with_key_embeds_marker_in_body() {
    let server = MockServer::start_async().await;
    let key = "idem-key-xyz789";
    let marker = format!("<!-- trogon-idempotency-key: {key} -->");

    // GET returns empty list — no prior comment.
    server.mock(|when, then| {
        when.method(httpmock::Method::GET)
            .path("/github/repos/owner/repo/issues/8/comments");
        then.status(200)
            .header("content-type", "application/json")
            .json_body(json!([]));
    });

    // POST must include the marker in the body.
    let post_mock = server.mock(|when, then| {
        when.method(httpmock::Method::POST)
            .path("/github/repos/owner/repo/issues/8/comments")
            .body_contains(&marker);
        then.status(201)
            .header("content-type", "application/json")
            .json_body(json!({ "html_url": "https://github.com/owner/repo/issues/8#comment-200" }));
    });

    let ctx = make_ctx(&server.base_url());
    let input = json!({
        "owner": "owner", "repo": "repo", "pr_number": 8,
        "body": "Looks good!", "_idempotency_key": key
    });
    let result = dispatch_tool(&ctx, "post_pr_comment", &input).await;

    assert!(result.contains("comment-200"), "got: {result}");
    assert_eq!(post_mock.hits(), 1, "POST must be called exactly once");
}

/// If the pre-post GET fails, `post_pr_comment` degrades gracefully and still
/// posts the comment (body unchanged since no key to embed a marker for).
/// This test uses `_idempotency_key` so the GET path is exercised; we force
/// the GET to return a non-JSON body to trigger the graceful-degradation path.
#[tokio::test]
async fn post_pr_comment_degrades_gracefully_when_get_fails() {
    let server = MockServer::start_async().await;
    let key = "idem-key-fail";
    let marker = format!("<!-- trogon-idempotency-key: {key} -->");

    // GET returns 500 — simulates proxy/network failure.
    server.mock(|when, then| {
        when.method(httpmock::Method::GET)
            .path("/github/repos/owner/repo/issues/9/comments");
        then.status(500).body("internal error");
    });

    // POST must still be called.
    let post_mock = server.mock(|when, then| {
        when.method(httpmock::Method::POST)
            .path("/github/repos/owner/repo/issues/9/comments")
            .body_contains(&marker);
        then.status(201)
            .header("content-type", "application/json")
            .json_body(json!({ "html_url": "https://github.com/owner/repo/issues/9#comment-300" }));
    });

    let ctx = make_ctx(&server.base_url());
    let input = json!({
        "owner": "owner", "repo": "repo", "pr_number": 9,
        "body": "fallback post", "_idempotency_key": key
    });
    let result = dispatch_tool(&ctx, "post_pr_comment", &input).await;

    assert!(result.contains("comment-300"), "got: {result}");
    assert_eq!(
        post_mock.hits(),
        1,
        "POST must still be called after GET failure"
    );
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

/// Without `_idempotency_key` the tool posts directly — no history check.
#[tokio::test]
async fn send_slack_message_calls_correct_proxy_path() {
    let server = MockServer::start_async().await;

    // No history mock: the tool must NOT call conversations.history.
    server.mock(|when, then| {
        when.method(httpmock::Method::POST)
            .path("/slack/chat.postMessage")
            .header("authorization", "Bearer tok_slack_prod_test01");
        then.status(200)
            .header("content-type", "application/json")
            .json_body(json!({ "ok": true, "ts": "1700000000.000001" }));
    });

    let ctx = make_ctx(&server.base_url());
    // No _idempotency_key → skip dedup check entirely.
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

    // No history mock: no _idempotency_key in input, so no check is done.
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

/// Primary dedup path: history contains a message whose `metadata` carries the
/// exact same idempotency key → skip send.
#[tokio::test]
async fn send_slack_message_skips_duplicate_by_metadata_key() {
    let server = MockServer::start_async().await;

    // History returns a message with matching metadata key (different text to
    // confirm that the metadata check fires before the text fallback).
    let history_mock = server.mock(|when, then| {
        when.method(httpmock::Method::GET)
            .path("/slack/conversations.history");
        then.status(200)
            .header("content-type", "application/json")
            .json_body(json!({
                "ok": true,
                "messages": [{
                    "ts": "1700000001.000001",
                    "user": "U123",
                    "text": "slightly different wording",
                    "metadata": {
                        "event_type": "trogon_agent_msg",
                        "event_payload": { "idempotency_key": "42.toolu_abc" }
                    }
                }]
            }));
    });

    // POST must NOT be called.
    let ctx = make_ctx(&server.base_url());
    let input = json!({
        "channel": "#engineering",
        "text": "PR #42 reviewed.",
        "_idempotency_key": "42.toolu_abc"
    });
    let result = dispatch_tool(&ctx, "send_slack_message", &input).await;

    history_mock.assert_hits_async(1).await;
    assert!(
        result.contains("skipped duplicate"),
        "unexpected result: {result}"
    );
}

/// Fallback dedup path: metadata scope unavailable (no `metadata` field in
/// history messages) but text matches → skip send.
#[tokio::test]
async fn send_slack_message_skips_duplicate_by_text_fallback_when_no_metadata() {
    let server = MockServer::start_async().await;

    // History has matching text but no metadata field (missing scope).
    let history_mock = server.mock(|when, then| {
        when.method(httpmock::Method::GET)
            .path("/slack/conversations.history");
        then.status(200)
            .header("content-type", "application/json")
            .json_body(json!({
                "ok": true,
                "messages": [
                    { "ts": "1700000001.000001", "user": "U123", "text": "PR #42 reviewed." }
                ]
            }));
    });

    // POST must NOT be called.
    let ctx = make_ctx(&server.base_url());
    let input = json!({
        "channel": "#engineering",
        "text": "PR #42 reviewed.",
        "_idempotency_key": "42.toolu_abc"
    });
    let result = dispatch_tool(&ctx, "send_slack_message", &input).await;

    history_mock.assert_hits_async(1).await;
    assert!(
        result.contains("skipped duplicate"),
        "unexpected result: {result}"
    );
}

/// Graceful degradation: history returns ok:false (e.g. missing channel scope)
/// → dedup check is skipped, send proceeds.
#[tokio::test]
async fn send_slack_message_sends_when_history_check_returns_error() {
    let server = MockServer::start_async().await;

    server.mock(|when, then| {
        when.method(httpmock::Method::GET)
            .path("/slack/conversations.history");
        then.status(200)
            .header("content-type", "application/json")
            .json_body(json!({ "ok": false, "error": "not_in_channel" }));
    });

    server.mock(|when, then| {
        when.method(httpmock::Method::POST)
            .path("/slack/chat.postMessage");
        then.status(200)
            .header("content-type", "application/json")
            .json_body(json!({ "ok": true, "ts": "1700000002.000002" }));
    });

    let ctx = make_ctx(&server.base_url());
    // Key present → triggers history check; history fails → proceeds.
    let input = json!({
        "channel": "#engineering",
        "text": "Deploy done.",
        "_idempotency_key": "99.toolu_xyz"
    });
    let result = dispatch_tool(&ctx, "send_slack_message", &input).await;

    assert!(
        result.contains("Message sent"),
        "unexpected result: {result}"
    );
}

/// Neither metadata nor text matches → send proceeds normally.
#[tokio::test]
async fn send_slack_message_sends_when_no_matching_message_in_history() {
    let server = MockServer::start_async().await;

    server.mock(|when, then| {
        when.method(httpmock::Method::GET)
            .path("/slack/conversations.history");
        then.status(200)
            .header("content-type", "application/json")
            .json_body(json!({
                "ok": true,
                "messages": [
                    { "ts": "1700000001.000001", "user": "U123", "text": "Some other message." }
                ]
            }));
    });

    server.mock(|when, then| {
        when.method(httpmock::Method::POST)
            .path("/slack/chat.postMessage");
        then.status(200)
            .header("content-type", "application/json")
            .json_body(json!({ "ok": true, "ts": "1700000003.000003" }));
    });

    let ctx = make_ctx(&server.base_url());
    let input = json!({
        "channel": "#engineering",
        "text": "New unique message.",
        "_idempotency_key": "55.toolu_new"
    });
    let result = dispatch_tool(&ctx, "send_slack_message", &input).await;

    assert!(
        result.contains("Message sent"),
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

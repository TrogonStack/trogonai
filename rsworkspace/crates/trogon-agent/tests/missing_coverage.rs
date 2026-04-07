//! Tests targeting previously uncovered code paths.
//!
//! Covers:
//! - `ContentBlock` serde tags (`type` field in JSON output)
//! - `pr_review::handle` with `reopened` action
//! - `post_pr_comment` fallback when response has no `html_url`
//! - `post_linear_comment` fallback when comment URL is absent
//! - `update_linear_issue` with `state_id` only
//! - `update_linear_issue` with all three fields simultaneously
//! - `list_pr_files` when proxy returns non-JSON body
//! - `get_pr_diff` when server returns 500 (body returned as text, not error)
//! - `get_linear_issue` when `data.issue` is null
//! - `Message::tool_results` with multiple results

use std::sync::Arc;

use httpmock::MockServer;
use serde_json::json;
use trogon_agent::{
    agent_loop::{AgentLoop, ContentBlock, Message, ReqwestAnthropicClient, ToolResult},
    flag_client::AlwaysOnFlagClient,
    handlers::{issue_triage, pr_review},
    tools::{DefaultToolDispatcher, ToolContext, dispatch_tool},
};

// ── helpers ───────────────────────────────────────────────────────────────────

fn make_ctx(proxy_url: &str) -> ToolContext {
    ToolContext {
        http_client: reqwest::Client::new(),
        proxy_url: proxy_url.to_string(),
        github_token: "tok_github_prod_test01".to_string(),
        linear_token: "tok_linear_prod_test01".to_string(),
        slack_token: String::new(),
    }
}

fn make_agent(proxy_url: &str) -> AgentLoop {
    let http_client = reqwest::Client::new();
    let tool_ctx = Arc::new(ToolContext {
        http_client: http_client.clone(),
        proxy_url: proxy_url.to_string(),
        github_token: "tok_github_prod_test01".to_string(),
        linear_token: "tok_linear_prod_test01".to_string(),
        slack_token: String::new(),
    });
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
    }
}

// ── ContentBlock serde tags ───────────────────────────────────────────────────

/// `ContentBlock::Text` serializes with `"type": "text"`.
#[test]
fn content_block_text_serializes_with_type_tag() {
    let block = ContentBlock::Text {
        text: "hello".to_string(),
    };
    let json = serde_json::to_value(&block).unwrap();
    assert_eq!(json["type"], "text");
    assert_eq!(json["text"], "hello");
}

/// `ContentBlock::ToolUse` serializes with `"type": "tool_use"`.
#[test]
fn content_block_tool_use_serializes_with_type_tag() {
    let block = ContentBlock::ToolUse {
        id: "call_abc".to_string(),
        name: "get_pr_diff".to_string(),
        input: json!({"owner": "org", "repo": "repo", "pr_number": 1}),
    };
    let json = serde_json::to_value(&block).unwrap();
    assert_eq!(json["type"], "tool_use");
    assert_eq!(json["id"], "call_abc");
    assert_eq!(json["name"], "get_pr_diff");
}

/// `ContentBlock::ToolResult` serializes with `"type": "tool_result"`.
#[test]
fn content_block_tool_result_serializes_with_type_tag() {
    let block = ContentBlock::ToolResult {
        tool_use_id: "call_abc".to_string(),
        content: "diff output".to_string(),
    };
    let json = serde_json::to_value(&block).unwrap();
    assert_eq!(json["type"], "tool_result");
    assert_eq!(json["tool_use_id"], "call_abc");
    assert_eq!(json["content"], "diff output");
}

/// `ContentBlock::Text` round-trips through JSON deserialize.
#[test]
fn content_block_text_deserializes_correctly() {
    let raw = json!({"type": "text", "text": "review done"});
    let block: ContentBlock = serde_json::from_value(raw).unwrap();
    assert!(matches!(block, ContentBlock::Text { text } if text == "review done"));
}

/// `ContentBlock::ToolUse` round-trips through JSON deserialize.
#[test]
fn content_block_tool_use_deserializes_correctly() {
    let raw = json!({
        "type": "tool_use",
        "id": "t1",
        "name": "list_pr_files",
        "input": {"owner": "o"}
    });
    let block: ContentBlock = serde_json::from_value(raw).unwrap();
    assert!(matches!(block, ContentBlock::ToolUse { id, .. } if id == "t1"));
}

// ── Message::tool_results with multiple results ───────────────────────────────

/// `Message::tool_results` with two results wraps both as `ToolResult` blocks.
#[test]
fn message_tool_results_with_multiple_entries() {
    let results = vec![
        ToolResult {
            tool_use_id: "id1".to_string(),
            content: "out1".to_string(),
        },
        ToolResult {
            tool_use_id: "id2".to_string(),
            content: "out2".to_string(),
        },
    ];
    let msg = Message::tool_results(results);
    let json = serde_json::to_value(&msg).unwrap();
    assert_eq!(json["role"], "user");
    let content = json["content"].as_array().unwrap();
    assert_eq!(content.len(), 2);
    assert_eq!(content[0]["type"], "tool_result");
    assert_eq!(content[0]["tool_use_id"], "id1");
    assert_eq!(content[1]["tool_use_id"], "id2");
}

// ── pr_review::handle — reopened ─────────────────────────────────────────────

/// `reopened` action triggers a review just like `opened`.
#[tokio::test]
async fn pr_review_handle_reopened_runs_agent() {
    let server = MockServer::start_async().await;

    server.mock(|when, then| {
        when.method(httpmock::Method::POST)
            .path("/anthropic/v1/messages");
        then.status(200)
            .header("content-type", "application/json")
            .json_body(json!({
                "stop_reason": "end_turn",
                "content": [{ "type": "text", "text": "Re-reviewing after reopen." }]
            }));
    });

    let payload = serde_json::to_vec(&json!({
        "action": "reopened",
        "number": 15,
        "repository": { "owner": { "login": "corp" }, "name": "svc" }
    }))
    .unwrap();

    let agent = make_agent(&server.base_url());
    let result = pr_review::handle(&agent, &payload).await;
    assert!(matches!(result, Some(Ok(ref s)) if s.contains("reopen")));
}

// ── post_pr_comment — no html_url fallback ────────────────────────────────────

/// When the GitHub response lacks `html_url`, the tool returns the "(no url)" fallback.
#[tokio::test]
async fn post_pr_comment_no_html_url_returns_fallback() {
    let server = MockServer::start_async().await;

    server.mock(|when, then| {
        when.method(httpmock::Method::POST)
            .path("/github/repos/org/repo/issues/42/comments");
        then.status(201)
            .header("content-type", "application/json")
            .json_body(json!({ "id": 999 })); // no html_url
    });

    let ctx = make_ctx(&server.base_url());
    let result = dispatch_tool(
        &ctx,
        "post_pr_comment",
        &json!({ "owner": "org", "repo": "repo", "pr_number": 42, "body": "LGTM" }),
    )
    .await;

    assert!(result.contains("(no url)"), "got: {result}");
}

// ── post_linear_comment — no url fallback ────────────────────────────────────

/// When the GraphQL response lacks the comment URL, the tool returns "(no url)".
#[tokio::test]
async fn post_linear_comment_no_url_returns_fallback() {
    let server = MockServer::start_async().await;

    server.mock(|when, then| {
        when.method(httpmock::Method::POST).path("/linear/graphql");
        then.status(200)
            .header("content-type", "application/json")
            .json_body(json!({
                "data": {
                    "commentCreate": {
                        "success": true,
                        "comment": {}  // no "url" field
                    }
                }
            }));
    });

    let ctx = make_ctx(&server.base_url());
    let result = dispatch_tool(
        &ctx,
        "post_linear_comment",
        &json!({ "issue_id": "ISS-1", "body": "triaged" }),
    )
    .await;

    assert!(result.contains("(no url)"), "got: {result}");
}

// ── update_linear_issue — state_id only ──────────────────────────────────────

/// A patch with only `state_id` sends only that field, not assignee or priority.
#[tokio::test]
async fn update_linear_issue_state_id_only() {
    let server = MockServer::start_async().await;

    let mock = server.mock(|when, then| {
        when.method(httpmock::Method::POST)
            .path("/linear/graphql")
            .body_contains("stateId")
            .body_contains("\"state-in-progress\"");
        then.status(200)
            .header("content-type", "application/json")
            .json_body(json!({
                "data": {
                    "issueUpdate": {
                        "success": true,
                        "issue": { "id": "ISS-5", "title": "Bug", "state": { "name": "In Progress" } }
                    }
                }
            }));
    });

    let ctx = make_ctx(&server.base_url());
    let result = dispatch_tool(
        &ctx,
        "update_linear_issue",
        &json!({ "issue_id": "ISS-5", "state_id": "state-in-progress" }),
    )
    .await;

    mock.assert_hits_async(1).await;
    assert!(result.contains("In Progress"), "got: {result}");
}

// ── update_linear_issue — all three fields ────────────────────────────────────

/// A patch with `state_id`, `assignee_id`, and `priority` all set sends all three.
#[tokio::test]
async fn update_linear_issue_all_three_fields() {
    let server = MockServer::start_async().await;

    let mock = server.mock(|when, then| {
        when.method(httpmock::Method::POST)
            .path("/linear/graphql")
            .body_contains("stateId")
            .body_contains("assigneeId")
            .body_contains("priority");
        then.status(200)
            .header("content-type", "application/json")
            .json_body(json!({
                "data": {
                    "issueUpdate": {
                        "success": true,
                        "issue": { "id": "ISS-7", "title": "Auth", "state": { "name": "Done" } }
                    }
                }
            }));
    });

    let ctx = make_ctx(&server.base_url());
    let result = dispatch_tool(
        &ctx,
        "update_linear_issue",
        &json!({
            "issue_id": "ISS-7",
            "state_id": "state-done",
            "assignee_id": "user-abc",
            "priority": 1
        }),
    )
    .await;

    mock.assert_hits_async(1).await;
    assert!(result.contains("Done"), "got: {result}");
}

// ── list_pr_files — non-JSON response ────────────────────────────────────────

/// When the proxy returns a plain-text body, `list_pr_files` propagates the
/// JSON parse error as a tool error string (not a panic).
#[tokio::test]
async fn list_pr_files_non_json_response_returns_error() {
    let server = MockServer::start_async().await;

    server.mock(|when, then| {
        when.method(httpmock::Method::GET)
            .path("/github/repos/org/repo/pulls/3/files");
        then.status(200)
            .header("content-type", "text/plain")
            .body("not json at all");
    });

    let ctx = make_ctx(&server.base_url());
    let result = dispatch_tool(
        &ctx,
        "list_pr_files",
        &json!({ "owner": "org", "repo": "repo", "pr_number": 3 }),
    )
    .await;

    assert!(result.starts_with("Tool error:"), "got: {result}");
}

// ── get_pr_diff — server 500 body returned as text ───────────────────────────

/// When the proxy returns 500, `get_pr_diff` returns the error body as a
/// plain string (`.text()` succeeds on any HTTP status).
#[tokio::test]
async fn get_pr_diff_server_error_returns_body_text() {
    let server = MockServer::start_async().await;

    server.mock(|when, then| {
        when.method(httpmock::Method::GET)
            .path("/github/repos/org/repo/pulls/99");
        then.status(500)
            .header("content-type", "text/plain")
            .body("Internal Server Error");
    });

    let ctx = make_ctx(&server.base_url());
    let result = dispatch_tool(
        &ctx,
        "get_pr_diff",
        &json!({ "owner": "org", "repo": "repo", "pr_number": 99 }),
    )
    .await;

    // Not a "Tool error:" — the body is returned as-is.
    assert!(!result.starts_with("Tool error:"), "got: {result}");
    assert!(result.contains("Internal Server Error"), "got: {result}");
}

// ── get_linear_issue — null issue in response ─────────────────────────────────

/// When the GraphQL response has `data.issue: null`, the tool serializes it
/// as the string `"null"`.
#[tokio::test]
async fn get_linear_issue_null_returns_null_string() {
    let server = MockServer::start_async().await;

    server.mock(|when, then| {
        when.method(httpmock::Method::POST).path("/linear/graphql");
        then.status(200)
            .header("content-type", "application/json")
            .json_body(json!({ "data": { "issue": null } }));
    });

    let ctx = make_ctx(&server.base_url());
    let result = dispatch_tool(
        &ctx,
        "get_linear_issue",
        &json!({ "issue_id": "NONEXISTENT-0" }),
    )
    .await;

    assert_eq!(result, "null", "got: {result}");
}

// ── issue_triage::handle — invalid json returns Some(Err) ────────────────────

/// The handler surfaces a parse error as `Some(Err(...))`, not a panic.
#[tokio::test]
async fn issue_triage_handle_invalid_json_returns_some_err() {
    let agent = make_agent("http://localhost:9999");
    let result = issue_triage::handle(&agent, b"{bad json}").await;
    assert!(matches!(result, Some(Err(_))));
}

// ── update_linear_issue — missing issue in success response ──────────────────

/// When `issueUpdate.issue` has no id/state, the fallback "?" strings are used.
#[tokio::test]
async fn update_linear_issue_missing_id_and_state_uses_fallback() {
    let server = MockServer::start_async().await;

    server.mock(|when, then| {
        when.method(httpmock::Method::POST).path("/linear/graphql");
        then.status(200)
            .header("content-type", "application/json")
            .json_body(json!({
                "data": {
                    "issueUpdate": {
                        "success": true,
                        "issue": {}  // no id, no state
                    }
                }
            }));
    });

    let ctx = make_ctx(&server.base_url());
    let result = dispatch_tool(
        &ctx,
        "update_linear_issue",
        &json!({ "issue_id": "ISS-X", "priority": 2 }),
    )
    .await;

    // Both id and state.name fall back to "?"
    assert!(result.contains("?"), "got: {result}");
}

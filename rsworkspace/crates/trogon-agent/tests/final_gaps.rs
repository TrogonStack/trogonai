//! Final gap coverage:
//!   - RunnerError display and Error impl
//!   - Message::assistant constructor
//!   - make_tool_context builds context correctly
//!   - Missing field errors for all remaining tools
//!   - get_file_contents error cases (invalid base64, missing content field)

use std::sync::Arc;

use base64::{Engine as _, engine::general_purpose};
use httpmock::MockServer;
use serde_json::json;
use trogon_agent::{
    RunnerError,
    agent_loop::{ContentBlock, Message},
    handlers::make_tool_context,
    tools::{ToolContext, dispatch_tool},
};

// ── RunnerError ───────────────────────────────────────────────────────────────

#[test]
fn runner_error_jetstream_display() {
    let e = RunnerError::JetStream("consumer create failed".to_string());
    assert!(e.to_string().contains("JetStream error"));
    assert!(e.to_string().contains("consumer create failed"));
}

#[test]
fn runner_error_jetstream_is_std_error() {
    let e = RunnerError::JetStream("x".to_string());
    let _: &dyn std::error::Error = &e;
}

// ── Message::assistant ────────────────────────────────────────────────────────

#[test]
fn message_assistant_has_correct_role_and_content() {
    let blocks = vec![ContentBlock::Text {
        text: "Hello".to_string(),
    }];
    let msg = Message::assistant(blocks.clone());
    // We can verify the message serializes with role=assistant.
    let json = serde_json::to_value(&msg).unwrap();
    assert_eq!(json["role"], "assistant");
    assert_eq!(json["content"][0]["type"], "text");
    assert_eq!(json["content"][0]["text"], "Hello");
}

#[test]
fn message_assistant_empty_content() {
    let msg = Message::assistant(vec![]);
    let json = serde_json::to_value(&msg).unwrap();
    assert_eq!(json["role"], "assistant");
    assert_eq!(json["content"].as_array().unwrap().len(), 0);
}

// ── make_tool_context ─────────────────────────────────────────────────────────

#[test]
fn make_tool_context_stores_all_fields() {
    let ctx: Arc<ToolContext> = make_tool_context(
        reqwest::Client::new(),
        "http://proxy:8080".to_string(),
        "tok_github_prod_abc".to_string(),
        "tok_linear_prod_abc".to_string(),
        String::new(),
    );
    assert_eq!(ctx.proxy_url, "http://proxy:8080");
    assert_eq!(ctx.github_token, "tok_github_prod_abc");
    assert_eq!(ctx.linear_token, "tok_linear_prod_abc");
}

// ── Missing field errors for remaining tools ──────────────────────────────────

fn dummy_ctx() -> ToolContext {
    ToolContext {
        http_client: reqwest::Client::new(),
        proxy_url: "http://localhost:9999".to_string(),
        github_token: String::new(),
        linear_token: String::new(),
        slack_token: String::new(),
    }
}

#[tokio::test]
async fn list_pr_files_missing_owner_returns_error() {
    let ctx = dummy_ctx();
    let result = dispatch_tool(
        &ctx,
        "list_pr_files",
        &json!({ "repo": "r", "pr_number": 1 }),
    )
    .await;
    assert!(result.contains("Tool error"), "got: {result}");
}

#[tokio::test]
async fn list_pr_files_missing_repo_returns_error() {
    let ctx = dummy_ctx();
    let result = dispatch_tool(
        &ctx,
        "list_pr_files",
        &json!({ "owner": "o", "pr_number": 1 }),
    )
    .await;
    assert!(result.contains("Tool error"), "got: {result}");
}

#[tokio::test]
async fn list_pr_files_missing_pr_number_returns_error() {
    let ctx = dummy_ctx();
    let result = dispatch_tool(&ctx, "list_pr_files", &json!({ "owner": "o", "repo": "r" })).await;
    assert!(result.contains("Tool error"), "got: {result}");
}

#[tokio::test]
async fn get_file_contents_missing_path_returns_error() {
    let ctx = dummy_ctx();
    let result = dispatch_tool(
        &ctx,
        "get_file_contents",
        &json!({ "owner": "o", "repo": "r" }),
    )
    .await;
    assert!(result.contains("Tool error"), "got: {result}");
}

#[tokio::test]
async fn post_pr_comment_missing_body_returns_error() {
    let ctx = dummy_ctx();
    let result = dispatch_tool(
        &ctx,
        "post_pr_comment",
        &json!({ "owner": "o", "repo": "r", "pr_number": 1 }),
    )
    .await;
    assert!(result.contains("Tool error"), "got: {result}");
}

#[tokio::test]
async fn get_linear_issue_missing_issue_id_returns_error() {
    let ctx = dummy_ctx();
    let result = dispatch_tool(&ctx, "get_linear_issue", &json!({})).await;
    assert!(result.contains("Tool error"), "got: {result}");
}

#[tokio::test]
async fn post_linear_comment_missing_body_returns_error() {
    let ctx = dummy_ctx();
    let result = dispatch_tool(&ctx, "post_linear_comment", &json!({ "issue_id": "I1" })).await;
    assert!(result.contains("Tool error"), "got: {result}");
}

#[tokio::test]
async fn update_linear_issue_missing_issue_id_returns_error() {
    let ctx = dummy_ctx();
    let result = dispatch_tool(&ctx, "update_linear_issue", &json!({})).await;
    assert!(result.contains("Tool error"), "got: {result}");
}

// ── get_file_contents error cases ─────────────────────────────────────────────

/// GitHub returns a response without the `content` field → tool error.
#[tokio::test]
async fn get_file_contents_missing_content_field_returns_error() {
    let server = MockServer::start_async().await;

    server.mock(|when, then| {
        when.method(httpmock::Method::GET)
            .path("/github/repos/o/r/contents/f.txt");
        then.status(200)
            .header("content-type", "application/json")
            .json_body(json!({ "encoding": "base64" })); // no "content" key
    });

    let ctx = ToolContext {
        http_client: reqwest::Client::new(),
        proxy_url: server.base_url(),
        github_token: "tok_github_prod_test01".to_string(),
        linear_token: String::new(),
        slack_token: String::new(),
    };
    let input = json!({ "owner": "o", "repo": "r", "path": "f.txt" });
    let result = dispatch_tool(&ctx, "get_file_contents", &input).await;
    assert!(result.contains("Tool error"), "got: {result}");
}

/// GitHub returns invalid base64 → tool error.
#[tokio::test]
async fn get_file_contents_invalid_base64_returns_error() {
    let server = MockServer::start_async().await;

    server.mock(|when, then| {
        when.method(httpmock::Method::GET)
            .path("/github/repos/o/r/contents/f.txt");
        then.status(200)
            .header("content-type", "application/json")
            .json_body(json!({ "content": "!!!not-valid-base64!!!" }));
    });

    let ctx = ToolContext {
        http_client: reqwest::Client::new(),
        proxy_url: server.base_url(),
        github_token: "tok_github_prod_test01".to_string(),
        linear_token: String::new(),
        slack_token: String::new(),
    };
    let input = json!({ "owner": "o", "repo": "r", "path": "f.txt" });
    let result = dispatch_tool(&ctx, "get_file_contents", &input).await;
    assert!(result.contains("Tool error"), "got: {result}");
}

/// GitHub returns base64 of non-UTF8 bytes → tool error.
#[tokio::test]
async fn get_file_contents_non_utf8_bytes_returns_error() {
    let server = MockServer::start_async().await;

    // Encode invalid UTF-8 bytes.
    let invalid_utf8 = vec![0xFF, 0xFE, 0x00];
    let encoded = general_purpose::STANDARD.encode(&invalid_utf8);

    server.mock(|when, then| {
        when.method(httpmock::Method::GET)
            .path("/github/repos/o/r/contents/binary.bin");
        then.status(200)
            .header("content-type", "application/json")
            .json_body(json!({ "content": encoded }));
    });

    let ctx = ToolContext {
        http_client: reqwest::Client::new(),
        proxy_url: server.base_url(),
        github_token: "tok_github_prod_test01".to_string(),
        linear_token: String::new(),
        slack_token: String::new(),
    };
    let input = json!({ "owner": "o", "repo": "r", "path": "binary.bin" });
    let result = dispatch_tool(&ctx, "get_file_contents", &input).await;
    assert!(result.contains("Tool error"), "got: {result}");
}

//! Gap coverage for GitHub and Linear tools:
//!   - get_file_contents with base64 that has embedded newlines (real GitHub format)
//!   - get_file_contents with explicit ref parameter
//!   - update_linear_issue failure (success: false)
//!   - dispatch_tool routing for all known tool names

use base64::{Engine as _, engine::general_purpose};
use httpmock::MockServer;
use serde_json::json;
use trogon_agent::tools::{ToolContext, dispatch_tool};

fn make_ctx(proxy_url: &str) -> ToolContext {
    ToolContext {
        http_client: reqwest::Client::new(),
        proxy_url: proxy_url.to_string(),
        github_token: "tok_github_prod_test01".to_string(),
        linear_token: "tok_linear_prod_test01".to_string(),
        slack_token: String::new(),
    }
}

// ── get_file_contents ─────────────────────────────────────────────────────────

/// GitHub returns base64 with `\n` every ~60 chars — the tool must strip them.
#[tokio::test]
async fn get_file_contents_strips_newlines_in_base64() {
    let content = "fn main() {\n    println!(\"hello world\");\n}";
    // Insert newlines into the base64 string every 20 chars (simulating GitHub).
    let raw_b64 = general_purpose::STANDARD.encode(content);
    let chunked: String = raw_b64
        .as_bytes()
        .chunks(20)
        .map(|c| std::str::from_utf8(c).unwrap())
        .collect::<Vec<_>>()
        .join("\n");

    let server = MockServer::start_async().await;

    server.mock(|when, then| {
        when.method(httpmock::Method::GET)
            .path("/github/repos/owner/repo/contents/src/main.rs");
        then.status(200)
            .header("content-type", "application/json")
            .json_body(json!({ "content": chunked, "encoding": "base64", "sha": "sha-chunked" }));
    });

    let ctx = make_ctx(&server.base_url());
    let input = json!({ "owner": "owner", "repo": "repo", "path": "src/main.rs" });
    let result = dispatch_tool(&ctx, "get_file_contents", &input).await;

    let parsed: serde_json::Value = serde_json::from_str(&result).expect("result must be JSON");
    assert_eq!(
        parsed["content"].as_str().unwrap(),
        content,
        "embedded newlines must be stripped before decoding"
    );
    assert_eq!(parsed["sha"].as_str().unwrap(), "sha-chunked");
}

/// When `ref` is provided it must appear in the query string.
#[tokio::test]
async fn get_file_contents_passes_ref_in_query() {
    let content = "version = 1";
    let encoded = general_purpose::STANDARD.encode(content);

    let server = MockServer::start_async().await;

    server.mock(|when, then| {
        when.method(httpmock::Method::GET)
            .path("/github/repos/owner/repo/contents/Cargo.toml")
            .query_param("ref", "main");
        then.status(200)
            .header("content-type", "application/json")
            .json_body(json!({ "content": encoded, "encoding": "base64", "sha": "sha-ref-main" }));
    });

    let ctx = make_ctx(&server.base_url());
    let input = json!({
        "owner": "owner",
        "repo": "repo",
        "path": "Cargo.toml",
        "ref": "main"
    });
    let result = dispatch_tool(&ctx, "get_file_contents", &input).await;

    let parsed: serde_json::Value = serde_json::from_str(&result).expect("result must be JSON");
    assert_eq!(parsed["content"].as_str().unwrap(), content);
    assert_eq!(parsed["sha"].as_str().unwrap(), "sha-ref-main");
}

/// When `ref` is omitted, the default `HEAD` is used in the query string.
#[tokio::test]
async fn get_file_contents_defaults_to_head_ref() {
    let encoded = general_purpose::STANDARD.encode("data");

    let server = MockServer::start_async().await;

    server.mock(|when, then| {
        when.method(httpmock::Method::GET)
            .path("/github/repos/o/r/contents/f.txt")
            .query_param("ref", "HEAD");
        then.status(200)
            .header("content-type", "application/json")
            .json_body(json!({ "content": encoded, "sha": "sha-head" }));
    });

    let ctx = make_ctx(&server.base_url());
    let input = json!({ "owner": "o", "repo": "r", "path": "f.txt" });
    let result = dispatch_tool(&ctx, "get_file_contents", &input).await;

    let parsed: serde_json::Value = serde_json::from_str(&result).expect("result must be JSON");
    assert_eq!(parsed["content"].as_str().unwrap(), "data");
    assert_eq!(parsed["sha"].as_str().unwrap(), "sha-head");
}

// ── update_linear_issue failure ───────────────────────────────────────────────

/// `issueUpdate.success = false` → tool returns an error string.
#[tokio::test]
async fn update_linear_issue_failure_returns_error() {
    let server = MockServer::start_async().await;

    server.mock(|when, then| {
        when.method(httpmock::Method::POST).path("/linear/graphql");
        then.status(200)
            .header("content-type", "application/json")
            .json_body(json!({
                "data": {
                    "issueUpdate": { "success": false }
                },
                "errors": [{ "message": "Not found" }]
            }));
    });

    let ctx = make_ctx(&server.base_url());
    let input = json!({ "issue_id": "ISS-99", "state_id": "done" });
    let result = dispatch_tool(&ctx, "update_linear_issue", &input).await;

    assert!(result.contains("Tool error"), "got: {result}");
}

/// Partial update: only `assignee_id` provided — other fields omitted.
#[tokio::test]
async fn update_linear_issue_partial_fields_assignee_only() {
    let server = MockServer::start_async().await;

    server.mock(|when, then| {
        when.method(httpmock::Method::POST).path("/linear/graphql");
        then.status(200)
            .header("content-type", "application/json")
            .json_body(json!({
                "data": {
                    "issueUpdate": {
                        "success": true,
                        "issue": { "id": "ISS-5", "title": "t", "state": { "name": "Todo" } }
                    }
                }
            }));
    });

    let ctx = make_ctx(&server.base_url());
    let input = json!({ "issue_id": "ISS-5", "assignee_id": "user-abc" });
    let result = dispatch_tool(&ctx, "update_linear_issue", &input).await;

    assert!(result.contains("ISS-5"), "got: {result}");
}

/// Partial update: only `priority` provided.
#[tokio::test]
async fn update_linear_issue_partial_fields_priority_only() {
    let server = MockServer::start_async().await;

    server.mock(|when, then| {
        when.method(httpmock::Method::POST).path("/linear/graphql");
        then.status(200)
            .header("content-type", "application/json")
            .json_body(json!({
                "data": {
                    "issueUpdate": {
                        "success": true,
                        "issue": { "id": "ISS-6", "title": "t", "state": { "name": "In Progress" } }
                    }
                }
            }));
    });

    let ctx = make_ctx(&server.base_url());
    let input = json!({ "issue_id": "ISS-6", "priority": 1 });
    let result = dispatch_tool(&ctx, "update_linear_issue", &input).await;

    assert!(result.contains("ISS-6"), "got: {result}");
}

// ── dispatch_tool routing ─────────────────────────────────────────────────────

/// `dispatch_tool` routes `get_pr_diff` to the GitHub proxy path.
#[tokio::test]
async fn dispatch_tool_routes_get_pr_diff() {
    let server = MockServer::start_async().await;

    let mock = server.mock(|when, then| {
        when.method(httpmock::Method::GET)
            .path("/github/repos/o/r/pulls/1");
        then.status(200)
            .header("content-type", "text/plain")
            .body("diff");
    });

    let ctx = make_ctx(&server.base_url());
    dispatch_tool(
        &ctx,
        "get_pr_diff",
        &json!({ "owner": "o", "repo": "r", "pr_number": 1 }),
    )
    .await;
    mock.assert_hits_async(1).await;
}

/// `dispatch_tool` routes `list_pr_files` to the correct GitHub path.
#[tokio::test]
async fn dispatch_tool_routes_list_pr_files() {
    let server = MockServer::start_async().await;

    let mock = server.mock(|when, then| {
        when.method(httpmock::Method::GET)
            .path("/github/repos/o/r/pulls/2/files");
        then.status(200)
            .header("content-type", "application/json")
            .json_body(json!([]));
    });

    let ctx = make_ctx(&server.base_url());
    dispatch_tool(
        &ctx,
        "list_pr_files",
        &json!({ "owner": "o", "repo": "r", "pr_number": 2 }),
    )
    .await;
    mock.assert_hits_async(1).await;
}

/// `dispatch_tool` routes `post_pr_comment` to the Issues comments path.
#[tokio::test]
async fn dispatch_tool_routes_post_pr_comment() {
    let server = MockServer::start_async().await;

    let mock = server.mock(|when, then| {
        when.method(httpmock::Method::POST)
            .path("/github/repos/o/r/issues/3/comments");
        then.status(201)
            .header("content-type", "application/json")
            .json_body(json!({ "html_url": "https://github.com/o/r/issues/3#c1" }));
    });

    let ctx = make_ctx(&server.base_url());
    dispatch_tool(
        &ctx,
        "post_pr_comment",
        &json!({
            "owner": "o", "repo": "r", "pr_number": 3, "body": "hi"
        }),
    )
    .await;
    mock.assert_hits_async(1).await;
}

/// `dispatch_tool` routes `get_linear_issue` to the Linear GraphQL path.
#[tokio::test]
async fn dispatch_tool_routes_get_linear_issue() {
    let server = MockServer::start_async().await;

    let mock = server.mock(|when, then| {
        when.method(httpmock::Method::POST).path("/linear/graphql");
        then.status(200)
            .header("content-type", "application/json")
            .json_body(json!({ "data": { "issue": { "id": "I1", "title": "t" } } }));
    });

    let ctx = make_ctx(&server.base_url());
    dispatch_tool(&ctx, "get_linear_issue", &json!({ "issue_id": "I1" })).await;
    mock.assert_hits_async(1).await;
}

/// `dispatch_tool` routes `post_linear_comment` to the Linear GraphQL path.
#[tokio::test]
async fn dispatch_tool_routes_post_linear_comment() {
    let server = MockServer::start_async().await;

    let mock = server.mock(|when, then| {
        when.method(httpmock::Method::POST).path("/linear/graphql");
        then.status(200)
            .header("content-type", "application/json")
            .json_body(json!({
                "data": {
                    "commentCreate": {
                        "success": true,
                        "comment": { "id": "c1", "url": "https://linear.app/c1" }
                    }
                }
            }));
    });

    let ctx = make_ctx(&server.base_url());
    dispatch_tool(
        &ctx,
        "post_linear_comment",
        &json!({ "issue_id": "I1", "body": "hi" }),
    )
    .await;
    mock.assert_hits_async(1).await;
}

/// `dispatch_tool` routes `update_linear_issue` to the Linear GraphQL path.
#[tokio::test]
async fn dispatch_tool_routes_update_linear_issue() {
    let server = MockServer::start_async().await;

    let mock = server.mock(|when, then| {
        when.method(httpmock::Method::POST).path("/linear/graphql");
        then.status(200)
            .header("content-type", "application/json")
            .json_body(json!({
                "data": {
                    "issueUpdate": {
                        "success": true,
                        "issue": { "id": "I2", "title": "t", "state": { "name": "Done" } }
                    }
                }
            }));
    });

    let ctx = make_ctx(&server.base_url());
    dispatch_tool(
        &ctx,
        "update_linear_issue",
        &json!({ "issue_id": "I2", "state_id": "s1" }),
    )
    .await;
    mock.assert_hits_async(1).await;
}

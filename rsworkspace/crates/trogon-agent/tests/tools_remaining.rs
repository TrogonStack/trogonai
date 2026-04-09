//! Happy-path unit tests for the four GitHub/Linear tools that only had
//! routing smoke-tests or Docker-based pipeline tests:
//!
//! - `get_pr_comments`     — GET  /github/repos/:owner/:repo/issues/:num/comments
//! - `update_file`         — PUT  /github/repos/:owner/:repo/contents/:path
//! - `create_pull_request` — POST /github/repos/:owner/:repo/pulls
//! - `get_linear_comments` — GraphQL query over /linear/graphql

use httpmock::MockServer;
use serde_json::json;
use trogon_agent::tools::{ToolContext, dispatch_tool};

fn make_ctx(proxy_url: &str) -> ToolContext {
    ToolContext::new(
        reqwest::Client::new(),
        proxy_url.to_string(),
        "tok_github_prod_test01".to_string(),
        "tok_linear_prod_test01".to_string(),
        String::new(),
    )
}

// ── get_pr_comments ───────────────────────────────────────────────────────────

#[tokio::test]
async fn get_pr_comments_calls_correct_proxy_path() {
    let server = MockServer::start_async().await;

    server.mock(|when, then| {
        when.method(httpmock::Method::GET)
            .path("/github/repos/acme/api/issues/42/comments")
            .header("authorization", "Bearer tok_github_prod_test01")
            .header("accept", "application/vnd.github.v3+json");
        then.status(200)
            .header("content-type", "application/json")
            .json_body(json!([
                { "id": 1, "user": { "login": "alice" }, "body": "LGTM!" },
                { "id": 2, "user": { "login": "bob" },   "body": "Nit: rename var" }
            ]));
    });

    let ctx = make_ctx(&server.base_url());
    let input = json!({ "owner": "acme", "repo": "api", "pr_number": 42 });
    let result = dispatch_tool(&ctx, "get_pr_comments", &input).await;

    assert!(
        result.contains("LGTM"),
        "expected comment body in result, got: {result}"
    );
    assert!(
        result.contains("alice"),
        "expected author in result, got: {result}"
    );
}

#[tokio::test]
async fn get_pr_comments_empty_list_returns_empty_array() {
    let server = MockServer::start_async().await;

    server.mock(|when, then| {
        when.method(httpmock::Method::GET)
            .path("/github/repos/acme/api/issues/1/comments");
        then.status(200)
            .header("content-type", "application/json")
            .json_body(json!([]));
    });

    let ctx = make_ctx(&server.base_url());
    let input = json!({ "owner": "acme", "repo": "api", "pr_number": 1 });
    let result = dispatch_tool(&ctx, "get_pr_comments", &input).await;

    assert!(result.contains("[]"), "expected empty array, got: {result}");
}

#[tokio::test]
async fn get_pr_comments_missing_owner_returns_error() {
    let server = MockServer::start_async().await;
    let ctx = make_ctx(&server.base_url());
    let input = json!({ "repo": "api", "pr_number": 1 });
    let result = dispatch_tool(&ctx, "get_pr_comments", &input).await;
    assert!(
        result.contains("missing owner"),
        "expected error, got: {result}"
    );
}

#[tokio::test]
async fn get_pr_comments_missing_pr_number_returns_error() {
    let server = MockServer::start_async().await;
    let ctx = make_ctx(&server.base_url());
    let input = json!({ "owner": "acme", "repo": "api" });
    let result = dispatch_tool(&ctx, "get_pr_comments", &input).await;
    assert!(
        result.contains("missing pr_number"),
        "expected error, got: {result}"
    );
}

// ── update_file ───────────────────────────────────────────────────────────────

#[tokio::test]
async fn update_file_calls_correct_proxy_path_and_base64_encodes_content() {
    let server = MockServer::start_async().await;

    server.mock(|when, then| {
        when.method(httpmock::Method::PUT)
            .path("/github/repos/acme/api/contents/.trogon/memory.md")
            .header("authorization", "Bearer tok_github_prod_test01")
            .header("accept", "application/vnd.github.v3+json")
            // content="hello" base64-encodes to "aGVsbG8="
            .body_contains("aGVsbG8=");
        then.status(200)
            .header("content-type", "application/json")
            .json_body(json!({ "commit": { "sha": "abc123def456" } }));
    });

    let ctx = make_ctx(&server.base_url());
    let input = json!({
        "owner": "acme", "repo": "api",
        "path": ".trogon/memory.md",
        "message": "Update memory",
        "content": "hello"
    });
    let result = dispatch_tool(&ctx, "update_file", &input).await;

    assert!(
        result.contains("abc123def456"),
        "expected commit sha, got: {result}"
    );
    assert!(
        result.contains(".trogon/memory.md"),
        "expected path, got: {result}"
    );
}

#[tokio::test]
async fn update_file_includes_sha_when_provided() {
    let server = MockServer::start_async().await;

    server.mock(|when, then| {
        when.method(httpmock::Method::PUT)
            .path("/github/repos/acme/api/contents/README.md")
            .body_contains("existing_sha_789");
        then.status(200)
            .header("content-type", "application/json")
            .json_body(json!({ "commit": { "sha": "newsha123" } }));
    });

    let ctx = make_ctx(&server.base_url());
    let input = json!({
        "owner": "acme", "repo": "api",
        "path": "README.md",
        "message": "Update readme",
        "content": "# Updated",
        "sha": "existing_sha_789"
    });
    let result = dispatch_tool(&ctx, "update_file", &input).await;
    assert!(
        result.contains("newsha123"),
        "expected commit sha, got: {result}"
    );
}

#[tokio::test]
async fn update_file_missing_content_returns_error() {
    let server = MockServer::start_async().await;
    let ctx = make_ctx(&server.base_url());
    let input = json!({ "owner": "acme", "repo": "api", "path": "f.md", "message": "m" });
    let result = dispatch_tool(&ctx, "update_file", &input).await;
    assert!(
        result.contains("missing content"),
        "expected error, got: {result}"
    );
}

#[tokio::test]
async fn update_file_missing_message_returns_error() {
    let server = MockServer::start_async().await;
    let ctx = make_ctx(&server.base_url());
    let input = json!({ "owner": "acme", "repo": "api", "path": "f.md", "content": "x" });
    let result = dispatch_tool(&ctx, "update_file", &input).await;
    assert!(
        result.contains("missing message"),
        "expected error, got: {result}"
    );
}

#[tokio::test]
async fn update_file_no_sha_omits_sha_field_in_body() {
    let server = MockServer::start_async().await;

    // If sha is sent the body would contain "sha" key — verify it's absent.
    let bad_mock = server.mock(|when, then| {
        when.method(httpmock::Method::PUT)
            .path("/github/repos/acme/api/contents/f.md")
            .body_contains("\"sha\"");
        then.status(500).body("sha must not be sent");
    });

    let ok_mock = server.mock(|when, then| {
        when.method(httpmock::Method::PUT)
            .path("/github/repos/acme/api/contents/f.md");
        then.status(200)
            .header("content-type", "application/json")
            .json_body(json!({ "commit": { "sha": "abc" } }));
    });

    let ctx = make_ctx(&server.base_url());
    let input = json!({
        "owner": "acme", "repo": "api", "path": "f.md",
        "message": "init", "content": "body"
        // no sha
    });
    let result = dispatch_tool(&ctx, "update_file", &input).await;
    assert!(result.contains("abc"), "expected ok result, got: {result}");
    assert_eq!(
        bad_mock.hits(),
        0,
        "sha must not appear in body when not provided"
    );
    let _ = ok_mock;
}

// ── create_pull_request ───────────────────────────────────────────────────────

#[tokio::test]
async fn create_pull_request_calls_correct_proxy_path() {
    let server = MockServer::start_async().await;

    server.mock(|when, then| {
        when.method(httpmock::Method::POST)
            .path("/github/repos/acme/api/pulls")
            .header("authorization", "Bearer tok_github_prod_test01")
            .header("accept", "application/vnd.github.v3+json")
            .body_contains("feature-branch")
            .body_contains("Add new feature");
        then.status(201)
            .header("content-type", "application/json")
            .json_body(json!({
                "number": 99,
                "html_url": "https://github.com/acme/api/pull/99"
            }));
    });

    let ctx = make_ctx(&server.base_url());
    let input = json!({
        "owner": "acme", "repo": "api",
        "title": "Add new feature",
        "head": "feature-branch",
        "base": "main",
        "body": "Implements the new feature."
    });
    let result = dispatch_tool(&ctx, "create_pull_request", &input).await;

    assert!(result.contains("#99"), "expected PR number, got: {result}");
    assert!(
        result.contains("https://github.com/acme/api/pull/99"),
        "expected URL, got: {result}"
    );
}

#[tokio::test]
async fn create_pull_request_defaults_base_to_main() {
    let server = MockServer::start_async().await;

    server.mock(|when, then| {
        when.method(httpmock::Method::POST)
            .path("/github/repos/acme/api/pulls")
            .body_contains("\"base\":\"main\"");
        then.status(201)
            .header("content-type", "application/json")
            .json_body(json!({ "number": 1, "html_url": "https://github.com/acme/api/pull/1" }));
    });

    let ctx = make_ctx(&server.base_url());
    // base not provided — should default to "main"
    let input = json!({ "owner": "acme", "repo": "api", "title": "T", "head": "feat" });
    let result = dispatch_tool(&ctx, "create_pull_request", &input).await;
    assert!(result.contains("#1"), "expected PR number, got: {result}");
}

#[tokio::test]
async fn create_pull_request_missing_title_returns_error() {
    let server = MockServer::start_async().await;
    let ctx = make_ctx(&server.base_url());
    let input = json!({ "owner": "acme", "repo": "api", "head": "feat" });
    let result = dispatch_tool(&ctx, "create_pull_request", &input).await;
    assert!(
        result.contains("missing title"),
        "expected error, got: {result}"
    );
}

#[tokio::test]
async fn create_pull_request_missing_head_returns_error() {
    let server = MockServer::start_async().await;
    let ctx = make_ctx(&server.base_url());
    let input = json!({ "owner": "acme", "repo": "api", "title": "T" });
    let result = dispatch_tool(&ctx, "create_pull_request", &input).await;
    assert!(
        result.contains("missing head"),
        "expected error, got: {result}"
    );
}

#[tokio::test]
async fn create_pull_request_no_html_url_in_response_uses_fallback() {
    let server = MockServer::start_async().await;

    server.mock(|when, then| {
        when.method(httpmock::Method::POST)
            .path("/github/repos/acme/api/pulls");
        then.status(201)
            .header("content-type", "application/json")
            .json_body(json!({ "number": 5 })); // no html_url
    });

    let ctx = make_ctx(&server.base_url());
    let input = json!({ "owner": "acme", "repo": "api", "title": "T", "head": "feat" });
    let result = dispatch_tool(&ctx, "create_pull_request", &input).await;
    assert!(
        result.contains("(no url)"),
        "expected fallback url, got: {result}"
    );
    assert!(result.contains("#5"), "expected PR number, got: {result}");
}

// ── get_linear_comments ───────────────────────────────────────────────────────

#[tokio::test]
async fn get_linear_comments_calls_graphql_and_returns_nodes() {
    let server = MockServer::start_async().await;

    server.mock(|when, then| {
        when.method(httpmock::Method::POST)
            .path("/linear/graphql")
            .header("authorization", "Bearer tok_linear_prod_test01")
            .body_contains("ISS-42");
        then.status(200)
            .header("content-type", "application/json")
            .json_body(json!({
                "data": {
                    "issue": {
                        "comments": {
                            "nodes": [
                                { "id": "c1", "body": "First comment", "createdAt": "2026-01-01T00:00:00Z",
                                  "user": { "name": "Alice", "email": "alice@acme.com" } },
                                { "id": "c2", "body": "Second comment", "createdAt": "2026-01-02T00:00:00Z",
                                  "user": { "name": "Bob", "email": "bob@acme.com" } }
                            ]
                        }
                    }
                }
            }));
    });

    let ctx = make_ctx(&server.base_url());
    let input = json!({ "issue_id": "ISS-42" });
    let result = dispatch_tool(&ctx, "get_linear_comments", &input).await;

    assert!(
        result.contains("First comment"),
        "expected comment body, got: {result}"
    );
    assert!(result.contains("Alice"), "expected author, got: {result}");
}

#[tokio::test]
async fn get_linear_comments_empty_nodes_returns_empty_array() {
    let server = MockServer::start_async().await;

    server.mock(|when, then| {
        when.method(httpmock::Method::POST).path("/linear/graphql");
        then.status(200)
            .header("content-type", "application/json")
            .json_body(json!({
                "data": { "issue": { "comments": { "nodes": [] } } }
            }));
    });

    let ctx = make_ctx(&server.base_url());
    let input = json!({ "issue_id": "ISS-1" });
    let result = dispatch_tool(&ctx, "get_linear_comments", &input).await;
    assert!(result.contains("[]"), "expected empty array, got: {result}");
}

#[tokio::test]
async fn get_linear_comments_missing_issue_id_returns_error() {
    let server = MockServer::start_async().await;
    let ctx = make_ctx(&server.base_url());
    let result = dispatch_tool(&ctx, "get_linear_comments", &json!({})).await;
    assert!(
        result.contains("missing issue_id"),
        "expected error, got: {result}"
    );
}

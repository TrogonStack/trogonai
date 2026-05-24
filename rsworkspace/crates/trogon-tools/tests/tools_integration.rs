//! Integration tests for `trogon-tools` via the public `dispatch_tool` API.
//!
//! These tests exercise the tools at the dispatch layer — verifying routing and
//! end-to-end behaviour that unit tests inside each module's `#[cfg(test)]` block
//! do not cover (particularly `.gitignore` filtering, which needs a real
//! directory tree with a `.gitignore` file and a live walker).
//!
//! Run with:
//!   cargo test -p trogon-tools --test tools_integration

use serde_json::json;
use tempfile::TempDir;
use trogon_tools::{ToolContext, dispatch_tool};

fn ctx(dir: &TempDir) -> ToolContext {
    ToolContext {
        proxy_url: String::new(),
        cwd: dir.path().to_string_lossy().into_owned(),
        http_client: reqwest::Client::new(),
    }
}

// ── helpers ───────────────────────────────────────────────────────────────────

/// Initialize a bare git repo in `dir` so `.gitignore` rules are respected by
/// the `ignore` crate's `WalkBuilder`. The crate only applies `.gitignore`
/// filtering when the directory is recognised as part of a git repository.
async fn git_init(dir: &TempDir) {
    tokio::process::Command::new("git")
        .args(["init", "-b", "main"])
        .current_dir(dir.path())
        .output()
        .await
        .expect("git init failed — is git installed?");
    tokio::process::Command::new("git")
        .args(["config", "user.email", "test@test.com"])
        .current_dir(dir.path())
        .output()
        .await
        .unwrap();
    tokio::process::Command::new("git")
        .args(["config", "user.name", "Test"])
        .current_dir(dir.path())
        .output()
        .await
        .unwrap();
}

// ── list_dir ──────────────────────────────────────────────────────────────────

/// `list_dir` must hide files listed in `.gitignore`.
///
/// The `ignore` crate's `WalkBuilder` applies gitignore rules only when the
/// directory is part of a git repository — hence `git_init()` is called first.
#[tokio::test]
async fn list_dir_respects_gitignore() {
    let dir = TempDir::new().unwrap();
    git_init(&dir).await;

    tokio::fs::write(dir.path().join(".gitignore"), "secret.log\n")
        .await
        .unwrap();
    tokio::fs::write(dir.path().join("secret.log"), "classified")
        .await
        .unwrap();
    tokio::fs::write(dir.path().join("visible.rs"), "pub fn main() {}")
        .await
        .unwrap();

    let result = dispatch_tool(&ctx(&dir), "list_dir", &json!({})).await;

    assert!(
        result.contains("visible.rs"),
        "visible.rs must appear in list_dir output; got: {result}"
    );
    assert!(
        !result.contains("secret.log"),
        "secret.log must be hidden by .gitignore; got: {result}"
    );
}

/// `list_dir` hides files matched by a glob pattern in `.gitignore`.
#[tokio::test]
async fn list_dir_respects_gitignore_glob_pattern() {
    let dir = TempDir::new().unwrap();
    git_init(&dir).await;

    tokio::fs::write(dir.path().join(".gitignore"), "*.log\n")
        .await
        .unwrap();
    tokio::fs::write(dir.path().join("app.log"), "log data").await.unwrap();
    tokio::fs::write(dir.path().join("app.log.bak"), "backup").await.unwrap();
    tokio::fs::write(dir.path().join("main.rs"), "fn main() {}").await.unwrap();

    let result = dispatch_tool(&ctx(&dir), "list_dir", &json!({})).await;

    assert!(result.contains("main.rs"), "main.rs must be listed; got: {result}");
    assert!(
        !result.contains("app.log\n") && !result.ends_with("app.log"),
        "app.log must be excluded by '*.log' pattern; got: {result}"
    );
    assert!(
        result.contains("app.log.bak"),
        "app.log.bak must be listed (not matched by *.log); got: {result}"
    );
}

// ── read_file ─────────────────────────────────────────────────────────────────

/// `read_file` dispatched via `dispatch_tool` must reject path traversal.
#[tokio::test]
async fn read_file_path_traversal_rejected_via_dispatch() {
    let dir = TempDir::new().unwrap();
    let result =
        dispatch_tool(&ctx(&dir), "read_file", &json!({"path": "../../etc/passwd"})).await;
    assert!(
        result.starts_with("Error"),
        "path traversal must be rejected; got: {result}"
    );
    assert!(
        result.contains("outside"),
        "error must mention 'outside'; got: {result}"
    );
}

/// `read_file` with `offset` and `limit` returns only the requested lines.
#[tokio::test]
async fn read_file_offset_and_limit_via_dispatch() {
    let dir = TempDir::new().unwrap();
    tokio::fs::write(dir.path().join("lines.txt"), "a\nb\nc\nd\ne\n")
        .await
        .unwrap();

    // offset=1 (0-indexed), limit=2 → lines 2 and 3
    let result = dispatch_tool(
        &ctx(&dir),
        "read_file",
        &json!({"path": "lines.txt", "offset": 1, "limit": 2}),
    )
    .await;

    assert!(result.contains("2\tb"), "line 2 ('b') must be present; got: {result}");
    assert!(result.contains("3\tc"), "line 3 ('c') must be present; got: {result}");
    assert!(!result.contains("1\ta"), "line 1 ('a') must NOT be present; got: {result}");
    assert!(!result.contains("4\td"), "line 4 ('d') must NOT be present; got: {result}");
}

// ── str_replace ───────────────────────────────────────────────────────────────

/// `str_replace` must return an error when `old_str` is not found (0 occurrences).
#[tokio::test]
async fn str_replace_zero_occurrences_rejected_via_dispatch() {
    let dir = TempDir::new().unwrap();
    tokio::fs::write(dir.path().join("f.rs"), "fn real() {}\n")
        .await
        .unwrap();

    let result = dispatch_tool(
        &ctx(&dir),
        "str_replace",
        &json!({"path": "f.rs", "old_str": "fn missing() {}", "new_str": "fn x() {}"}),
    )
    .await;

    assert!(
        result.contains("0 occurrences"),
        "must report 0 occurrences; got: {result}"
    );
}

/// `str_replace` must return an error when `old_str` appears more than once.
#[tokio::test]
async fn str_replace_duplicate_occurrences_rejected_via_dispatch() {
    let dir = TempDir::new().unwrap();
    tokio::fs::write(dir.path().join("f.rs"), "todo!()\ntodo!()\n")
        .await
        .unwrap();

    let result = dispatch_tool(
        &ctx(&dir),
        "str_replace",
        &json!({"path": "f.rs", "old_str": "todo!()", "new_str": "unimplemented!()"}),
    )
    .await;

    assert!(
        result.contains("2 occurrences"),
        "must report 2 occurrences; got: {result}"
    );
}

// ── fetch_url ─────────────────────────────────────────────────────────────────

/// `fetch_url` via dispatch must convert HTML to plain text by default.
#[tokio::test]
async fn fetch_url_html_to_text_via_dispatch() {
    use httpmock::prelude::*;

    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(GET).path("/page");
        then.status(200)
            .header("content-type", "text/html")
            .body("<html><body><p>Hello integration world</p></body></html>");
    });

    let dir = TempDir::new().unwrap();
    let result = dispatch_tool(
        &ctx(&dir),
        "fetch_url",
        &json!({"url": server.url("/page")}),
    )
    .await;

    assert!(
        result.contains("Hello integration world"),
        "HTML body must be converted to plain text; got: {result}"
    );
    assert!(
        !result.contains("<p>"),
        "HTML tags must be stripped; got: {result}"
    );
}

/// `fetch_url` via dispatch truncates responses larger than 8 KB.
#[tokio::test]
async fn fetch_url_truncates_large_response_via_dispatch() {
    use httpmock::prelude::*;

    let server = MockServer::start();
    let big_body = "x".repeat(8 * 1024 + 200);
    server.mock(|when, then| {
        when.method(GET).path("/big");
        then.status(200).body(big_body);
    });

    let dir = TempDir::new().unwrap();
    let result = dispatch_tool(
        &ctx(&dir),
        "fetch_url",
        &json!({"url": server.url("/big"), "raw": true}),
    )
    .await;

    assert!(
        result.contains("truncated at 8KB"),
        "large response must be truncated; got: {result}"
    );
}

// ── git_diff truncation ───────────────────────────────────────────────────────

/// `git_diff` via dispatch truncates output larger than 4 KB.
#[tokio::test]
async fn git_diff_truncates_large_output_via_dispatch() {
    let dir = TempDir::new().unwrap();

    tokio::process::Command::new("git")
        .args(["init", "-b", "main"])
        .current_dir(dir.path())
        .output()
        .await
        .unwrap();
    tokio::process::Command::new("git")
        .args(["config", "user.email", "test@test.com"])
        .current_dir(dir.path())
        .output()
        .await
        .unwrap();
    tokio::process::Command::new("git")
        .args(["config", "user.name", "Test"])
        .current_dir(dir.path())
        .output()
        .await
        .unwrap();

    let big_content = "x".repeat(8 * 1024);
    tokio::fs::write(dir.path().join("big.txt"), &big_content)
        .await
        .unwrap();
    tokio::process::Command::new("git")
        .args(["add", "big.txt"])
        .current_dir(dir.path())
        .output()
        .await
        .unwrap();

    let result = dispatch_tool(
        &ctx(&dir),
        "git_diff",
        &json!({"args": "--staged"}),
    )
    .await;

    assert!(
        result.contains("truncated at 4KB"),
        "large git diff must be truncated at 4KB; got: {result}"
    );
}

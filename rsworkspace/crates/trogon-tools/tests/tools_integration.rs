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

// ── write_file ────────────────────────────────────────────────────────────────

/// `write_file` dispatched via `dispatch_tool` creates the file on disk and
/// returns "OK".
#[tokio::test]
async fn write_file_creates_file_and_returns_ok_via_dispatch() {
    let dir = TempDir::new().unwrap();

    let result = dispatch_tool(
        &ctx(&dir),
        "write_file",
        &json!({"path": "output.rs", "content": "fn main() {}\n"}),
    )
    .await;

    assert_eq!(result, "OK", "write_file must return 'OK'; got: {result}");

    let on_disk = tokio::fs::read_to_string(dir.path().join("output.rs"))
        .await
        .unwrap();
    assert_eq!(on_disk, "fn main() {}\n", "file content must match what was written");
}

// ── str_replace (happy path) ──────────────────────────────────────────────────

/// `str_replace` dispatched via `dispatch_tool` must apply the edit when
/// `old_str` appears exactly once and return a diff-style context.
#[tokio::test]
async fn str_replace_applies_edit_via_dispatch() {
    let dir = TempDir::new().unwrap();
    tokio::fs::write(dir.path().join("src.rs"), "fn old_name() {}\n")
        .await
        .unwrap();

    let result = dispatch_tool(
        &ctx(&dir),
        "str_replace",
        &json!({"path": "src.rs", "old_str": "fn old_name()", "new_str": "fn new_name()"}),
    )
    .await;

    assert!(
        !result.starts_with("Error"),
        "str_replace must succeed; got: {result}"
    );

    let on_disk = tokio::fs::read_to_string(dir.path().join("src.rs"))
        .await
        .unwrap();
    assert!(
        on_disk.contains("fn new_name()"),
        "file must contain the replacement; got: {on_disk}"
    );
    assert!(
        !on_disk.contains("fn old_name()"),
        "file must not contain old text; got: {on_disk}"
    );
}

// ── write_file: intermediate directory creation ───────────────────────────────

/// `write_file` must create all intermediate directories when the target path
/// contains subdirectories that do not yet exist (spec: `create_dir_all`).
#[tokio::test]
async fn write_file_creates_intermediate_directories_via_dispatch() {
    let dir = TempDir::new().unwrap();

    let result = dispatch_tool(
        &ctx(&dir),
        "write_file",
        &json!({"path": "a/b/c/nested.rs", "content": "fn main() {}\n"}),
    )
    .await;

    assert_eq!(result, "OK", "write_file must return 'OK' for nested path; got: {result}");

    let on_disk = tokio::fs::read_to_string(dir.path().join("a/b/c/nested.rs"))
        .await
        .expect("file at nested path must exist after write_file with create_dir_all");
    assert_eq!(on_disk, "fn main() {}\n");
}

// ── list_dir: 500-entry truncation ────────────────────────────────────────────

/// `list_dir` must truncate output when the directory contains more than 500
/// entries and include the truncation notice in the returned string.
#[tokio::test]
async fn list_dir_truncates_at_500_entries_via_dispatch() {
    let dir = TempDir::new().unwrap();

    for i in 0u32..502 {
        tokio::fs::write(dir.path().join(format!("f{i:04}.txt")), "").await.unwrap();
    }

    let result = dispatch_tool(&ctx(&dir), "list_dir", &json!({})).await;

    assert!(
        result.contains("truncated at 500 entries"),
        "list_dir must include truncation notice when >500 entries; got: {result}"
    );
}

// ── glob ──────────────────────────────────────────────────────────────────────

/// `glob` via `dispatch_tool` matches files recursively across subdirectories
/// and excludes files that do not match the pattern.
#[tokio::test]
async fn glob_matches_files_recursively_via_dispatch() {
    let dir = TempDir::new().unwrap();

    tokio::fs::create_dir_all(dir.path().join("src/nested")).await.unwrap();
    tokio::fs::write(dir.path().join("src/main.rs"), "fn main() {}").await.unwrap();
    tokio::fs::write(dir.path().join("src/nested/util.rs"), "fn util() {}").await.unwrap();
    tokio::fs::write(dir.path().join("build.sh"), "#!/bin/sh").await.unwrap();

    let result = dispatch_tool(&ctx(&dir), "glob", &json!({"pattern": "**/*.rs"})).await;

    assert!(
        result.contains("main.rs"),
        "glob must find main.rs recursively; got: {result}"
    );
    assert!(
        result.contains("util.rs"),
        "glob must find nested util.rs; got: {result}"
    );
    assert!(
        !result.contains("build.sh"),
        "glob must not include build.sh (wrong extension); got: {result}"
    );
}

// ── git_status ────────────────────────────────────────────────────────────────

/// `git_status` via `dispatch_tool` lists untracked files present in the repo.
#[tokio::test]
async fn git_status_shows_untracked_file_via_dispatch() {
    let dir = TempDir::new().unwrap();
    git_init(&dir).await;

    tokio::fs::write(dir.path().join("status_sentinel.rs"), "fn main() {}").await.unwrap();

    let result = dispatch_tool(&ctx(&dir), "git_status", &json!({})).await;

    assert!(
        result.contains("status_sentinel.rs"),
        "git_status must list status_sentinel.rs as untracked; got: {result}"
    );
}

// ── git_log ───────────────────────────────────────────────────────────────────

/// `git_log` via `dispatch_tool` returns the commit message of the most recent commit.
#[tokio::test]
async fn git_log_shows_commit_message_via_dispatch() {
    let dir = TempDir::new().unwrap();
    git_init(&dir).await;

    tokio::fs::write(dir.path().join("README.md"), "# project\n").await.unwrap();
    tokio::process::Command::new("git")
        .args(["add", "README.md"])
        .current_dir(dir.path())
        .output()
        .await
        .unwrap();
    tokio::process::Command::new("git")
        .args(["commit", "-m", "log-sentinel-commit-abc123"])
        .current_dir(dir.path())
        .output()
        .await
        .unwrap();

    let result = dispatch_tool(&ctx(&dir), "git_log", &json!({})).await;

    assert!(
        result.contains("log-sentinel-commit-abc123"),
        "git_log must include the commit message; got: {result}"
    );
}

// ── notebook_edit ─────────────────────────────────────────────────────────────

/// `notebook_edit` via `dispatch_tool` writes the updated cell source back to
/// disk — verified by reading the `.ipynb` file after the edit completes.
#[tokio::test]
async fn notebook_edit_writes_new_cell_source_to_disk_via_dispatch() {
    let dir = TempDir::new().unwrap();
    let nb = serde_json::json!({
        "nbformat": 4,
        "nbformat_minor": 5,
        "metadata": {},
        "cells": [
            {
                "cell_type": "code",
                "source": ["print('original')"],
                "metadata": {},
                "outputs": [],
                "execution_count": null
            }
        ]
    });
    tokio::fs::write(
        dir.path().join("notebook.ipynb"),
        serde_json::to_string(&nb).unwrap(),
    )
    .await
    .unwrap();

    let result = dispatch_tool(
        &ctx(&dir),
        "notebook_edit",
        &json!({"path": "notebook.ipynb", "cell_index": 0, "content": "print('edited-sentinel')"}),
    )
    .await;

    assert_eq!(result, "OK", "notebook_edit must return 'OK'; got: {result}");

    let raw = tokio::fs::read_to_string(dir.path().join("notebook.ipynb")).await.unwrap();
    let on_disk: serde_json::Value = serde_json::from_str(&raw).unwrap();
    let source = on_disk["cells"][0]["source"].to_string();
    assert!(
        source.contains("edited-sentinel"),
        "edited cell source must be written to disk; got: {source}"
    );
    assert!(
        !source.contains("original"),
        "old cell source must be gone after notebook_edit; got: {source}"
    );
}

// ── search_files ──────────────────────────────────────────────────────────────

/// `search_files` via `dispatch_tool` returns the file paths that contain the
/// search pattern and excludes files without a match.
#[tokio::test]
async fn search_files_returns_matching_lines_via_dispatch() {
    let dir = TempDir::new().unwrap();
    tokio::fs::write(
        dir.path().join("haystack.rs"),
        "fn search_needle_sentinel() {}\nfn other() {}\n",
    )
    .await
    .unwrap();
    tokio::fs::write(dir.path().join("empty.rs"), "fn no_match() {}\n")
        .await
        .unwrap();

    let result =
        dispatch_tool(&ctx(&dir), "search_files", &json!({"pattern": "search_needle_sentinel"}))
            .await;

    assert!(
        result.contains("haystack.rs"),
        "search_files must report haystack.rs; got: {result}"
    );
    assert!(
        !result.contains("empty.rs"),
        "search_files must not report empty.rs (no match); got: {result}"
    );
    assert!(
        result.contains("search_needle_sentinel"),
        "search_files must include the matching text in output; got: {result}"
    );
}

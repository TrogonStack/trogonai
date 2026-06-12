use serde_json::Value;

use crate::ToolContext;
use crate::fs::resolve_path;

const MAX_OUTPUT: usize = 4 * 1024;
const MAX_COMMIT_MESSAGE_BYTES: usize = 10_000;
/// B12: hard cap on how much git stdout we buffer in memory before killing the
/// child. Comfortably above MAX_OUTPUT so normal output is unaffected, but bounded
/// so a multi-gigabyte diff can't exhaust RAM.
const MAX_STDOUT_BUFFER: usize = 256 * 1024;

async fn run_git(cwd: &str, args: &[&str]) -> String {
    use tokio::io::AsyncReadExt;
    use tokio::process::Command;

    let mut child = match Command::new("git")
        .args(args)
        .current_dir(cwd)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => return format!("Error running git: {e}"),
    };

    // B12: stream stdout and stop once we exceed the in-memory cap so a huge repo
    // diff can't buffer unbounded. Reading one byte past the cap lets us detect
    // (and flag) truncation before we kill the child.
    let mut stdout_buf = Vec::new();
    let mut truncated = false;
    if let Some(mut out) = child.stdout.take() {
        let mut chunk = [0u8; 8 * 1024];
        loop {
            match out.read(&mut chunk).await {
                Ok(0) => break,
                Ok(n) => {
                    stdout_buf.extend_from_slice(&chunk[..n]);
                    if stdout_buf.len() > MAX_STDOUT_BUFFER {
                        truncated = true;
                        let _ = child.start_kill();
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    }

    let mut stderr_buf = Vec::new();
    if let Some(mut err) = child.stderr.take() {
        // stderr is git's diagnostic channel — small; cap it at MAX_STDOUT_BUFFER too.
        let mut chunk = [0u8; 8 * 1024];
        while let Ok(n) = err.read(&mut chunk).await {
            if n == 0 {
                break;
            }
            stderr_buf.extend_from_slice(&chunk[..n]);
            if stderr_buf.len() > MAX_STDOUT_BUFFER {
                break;
            }
        }
    }

    let _ = child.wait().await;

    let stdout = String::from_utf8_lossy(&stdout_buf);
    let stderr = String::from_utf8_lossy(&stderr_buf);
    let combined = if stderr.is_empty() {
        stdout.into_owned()
    } else if stdout.is_empty() {
        stderr.into_owned()
    } else {
        format!("{stdout}{stderr}")
    };
    if combined.is_empty() {
        "(no output)".to_string()
    } else if combined.len() > MAX_OUTPUT || truncated {
        let cap = MAX_OUTPUT.min(combined.len());
        let boundary = combined.floor_char_boundary(cap);
        format!("{}... (truncated at 4KB)", &combined[..boundary])
    } else {
        combined
    }
}

pub async fn status(ctx: &ToolContext, _input: &Value) -> String {
    run_git(&ctx.cwd, &["status", "--short"]).await
}

pub async fn diff(ctx: &ToolContext, input: &Value) -> String {
    let mut args = vec!["diff"];
    let extra = input.get("args").and_then(|v| v.as_str());
    // B13: split on whitespace so `args: "--staged HEAD~1"` becomes two argv
    // elements instead of one broken arg.
    if let Some(e) = extra {
        args.extend(e.split_whitespace());
    }
    run_git(&ctx.cwd, &args).await
}

pub async fn log(ctx: &ToolContext, _input: &Value) -> String {
    run_git(&ctx.cwd, &["log", "--oneline", "-20"]).await
}

/// Create a new branch, optionally checking it out.
pub async fn create_branch(ctx: &ToolContext, input: &Value) -> String {
    let branch = match input.get("branch").and_then(|v| v.as_str()) {
        Some(b) if !b.trim().is_empty() => b,
        _ => return "Error: `branch` is required for git_create_branch".to_string(),
    };
    if branch.contains('\0') {
        return "Error: branch name must not contain NUL bytes".to_string();
    }

    let checkout = input.get("checkout").and_then(|v| v.as_bool()).unwrap_or(true);
    let base = input.get("base").and_then(|v| v.as_str());

    if checkout {
        let mut args = vec!["checkout", "-b", branch];
        if let Some(b) = base {
            args.push(b);
        }
        run_git(&ctx.cwd, &args).await
    } else {
        let mut args = vec!["branch", branch];
        if let Some(b) = base {
            args.push(b);
        }
        run_git(&ctx.cwd, &args).await
    }
}

/// Push the current branch (or a named branch) to a remote.
pub async fn push(ctx: &ToolContext, input: &Value) -> String {
    let remote = input
        .get("remote")
        .and_then(|v| v.as_str())
        .unwrap_or("origin");
    let branch = input.get("branch").and_then(|v| v.as_str());
    let set_upstream = input
        .get("set_upstream")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    let mut args = vec!["push"];
    if set_upstream {
        args.push("-u");
    }
    args.push(remote);
    if let Some(b) = branch {
        args.push(b);
    }
    run_git(&ctx.cwd, &args).await
}

/// Stage changed files and create a commit. Requires a non-empty `message`.
pub async fn commit(ctx: &ToolContext, input: &Value) -> String {
    let message = match input.get("message").and_then(|v| v.as_str()) {
        Some(m) if !m.trim().is_empty() => m,
        _ => return "Error: `message` is required for git_commit".to_string(),
    };
    if message.contains('\0') {
        return "Error: commit message must not contain NUL bytes".to_string();
    }
    if message.len() > MAX_COMMIT_MESSAGE_BYTES {
        return format!(
            "Error: commit message too long ({} bytes, max {MAX_COMMIT_MESSAGE_BYTES})",
            message.len()
        );
    }

    if input.get("all").and_then(|v| v.as_bool()) == Some(true) {
        let staged = run_git(&ctx.cwd, &["add", "-A"]).await;
        if staged.starts_with("Error running git") {
            return staged;
        }
    } else if let Some(paths) = input.get("paths").and_then(|v| v.as_array()) {
        for path in paths {
            let Some(p) = path.as_str() else {
                return "Error: `paths` must be an array of strings".to_string();
            };
            // MED-16: refuse to stage paths that escape the working directory
            // (e.g. `../sibling-repo/credentials.json`) even if git tracks them.
            if let Err(e) = resolve_path(&ctx.cwd, p) {
                return format!("Error: refusing to stage path outside working directory: {p} ({e})");
            }
            let added = run_git(&ctx.cwd, &["add", "--", p]).await;
            if added.starts_with("Error running git") {
                return added;
            }
        }
    }

    run_git(&ctx.cwd, &["commit", "-m", message]).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tempfile::TempDir;

    fn ctx() -> ToolContext {
        ToolContext {
            proxy_url: String::new(),
            cwd: std::env::current_dir()
                .unwrap()
                .to_string_lossy()
                .into_owned(),
            http_client: reqwest::Client::new(),
            web_search_api_key: None,
            web_search_endpoint: None,
        }
    }

    #[tokio::test]
    async fn git_status_runs_without_error() {
        let result = status(&ctx(), &json!({})).await;
        assert!(!result.starts_with("Error running git"));
    }

    #[tokio::test]
    async fn git_log_returns_commits() {
        let result = log(&ctx(), &json!({})).await;
        assert!(!result.starts_with("Error running git"));
    }

    #[tokio::test]
    async fn git_diff_runs_without_error() {
        let result = diff(&ctx(), &json!({})).await;
        assert!(!result.starts_with("Error running git"));
    }

    #[tokio::test]
    async fn git_diff_accepts_extra_args() {
        let result = diff(&ctx(), &json!({"args": "--staged"})).await;
        assert!(!result.starts_with("Error running git"));
    }

    #[tokio::test]
    async fn git_diff_splits_multiple_args() {
        // B13: "--staged --stat" must become two argv elements, not one broken arg.
        let dir = TempDir::new().unwrap();
        init_repo(dir.path()).await;
        tokio::fs::write(dir.path().join("a.txt"), "hello\n").await.unwrap();
        tokio::process::Command::new("git")
            .args(["add", "a.txt"])
            .current_dir(dir.path())
            .output()
            .await
            .unwrap();
        let ctx = ctx_in(dir.path());
        let result = diff(&ctx, &json!({"args": "--staged --stat"})).await;
        // A single broken arg "--staged --stat" would error; split args succeed.
        assert!(!result.starts_with("Error"), "got: {result}");
        assert!(result.contains("a.txt"), "got: {result}");
    }

    #[tokio::test]
    async fn git_diff_truncates_large_output() {
        use tempfile::TempDir;
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

        let large_ctx = ToolContext {
            proxy_url: String::new(),
            cwd: dir.path().to_string_lossy().into_owned(),
            http_client: reqwest::Client::new(),
            web_search_api_key: None,
            web_search_endpoint: None,
        };
        let result = diff(&large_ctx, &json!({"args": "--staged"})).await;
        assert!(result.contains("truncated at 4KB"), "expected truncation, got: {result}");
    }

    #[tokio::test]
    async fn git_commit_requires_message() {
        let dir = TempDir::new().unwrap();
        init_repo(dir.path()).await;
        let ctx = ctx_in(dir.path());
        let result = commit(&ctx, &json!({})).await;
        assert!(result.contains("message"), "got: {result}");
    }

    #[tokio::test]
    async fn git_commit_stages_and_commits() {
        let dir = TempDir::new().unwrap();
        init_repo(dir.path()).await;
        tokio::fs::write(dir.path().join("a.txt"), "hello").await.unwrap();
        let ctx = ctx_in(dir.path());
        let result = commit(
            &ctx,
            &json!({"message": "add a", "paths": ["a.txt"]}),
        )
        .await;
        assert!(!result.starts_with("Error"), "got: {result}");
        let log_out = log(&ctx, &json!({})).await;
        assert!(log_out.contains("add a"), "got: {log_out}");
    }

    async fn init_repo(path: &std::path::Path) {
        for args in [
            &["init", "-b", "main"][..],
            &["config", "user.email", "test@test.com"][..],
            &["config", "user.name", "Test"][..],
        ] {
            tokio::process::Command::new("git")
                .args(args)
                .current_dir(path)
                .output()
                .await
                .unwrap();
        }
    }

    fn ctx_in(path: &std::path::Path) -> ToolContext {
        ToolContext {
            proxy_url: String::new(),
            cwd: path.to_string_lossy().into_owned(),
            http_client: reqwest::Client::new(),
            web_search_api_key: None,
            web_search_endpoint: None,
        }
    }

    #[tokio::test]
    async fn git_create_branch_requires_branch_name() {
        let dir = TempDir::new().unwrap();
        init_repo(dir.path()).await;
        let ctx = ctx_in(dir.path());
        let result = create_branch(&ctx, &json!({})).await;
        assert!(result.contains("branch"), "got: {result}");
    }

    #[tokio::test]
    async fn git_create_branch_and_checkout() {
        let dir = TempDir::new().unwrap();
        init_repo(dir.path()).await;
        // need an initial commit so branch creation works
        tokio::fs::write(dir.path().join("init.txt"), "init").await.unwrap();
        let ctx = ctx_in(dir.path());
        commit(&ctx, &json!({"message": "init", "paths": ["init.txt"]})).await;

        let result = create_branch(&ctx, &json!({"branch": "feat/test", "checkout": true})).await;
        assert!(!result.starts_with("Error"), "got: {result}");
        // verify we are now on the new branch
        let status = run_git(dir.path().to_str().unwrap(), &["branch", "--show-current"]).await;
        assert_eq!(status.trim(), "feat/test", "got: {status}");
    }

    #[tokio::test]
    async fn git_create_branch_without_checkout() {
        let dir = TempDir::new().unwrap();
        init_repo(dir.path()).await;
        tokio::fs::write(dir.path().join("init.txt"), "init").await.unwrap();
        let ctx = ctx_in(dir.path());
        commit(&ctx, &json!({"message": "init", "paths": ["init.txt"]})).await;

        let result = create_branch(&ctx, &json!({"branch": "no-checkout", "checkout": false})).await;
        assert!(!result.starts_with("Error"), "got: {result}");
        // still on original branch
        let current = run_git(dir.path().to_str().unwrap(), &["branch", "--show-current"]).await;
        assert_eq!(current.trim(), "main", "got: {current}");
    }

    #[tokio::test]
    async fn git_push_requires_remote_reachable() {
        // Just verify the function doesn't panic on a repo with no remote.
        // git push will fail with an error message — that's expected.
        let dir = TempDir::new().unwrap();
        init_repo(dir.path()).await;
        let ctx = ctx_in(dir.path());
        let result = push(&ctx, &json!({})).await;
        // Either "no remote" / "does not have upstream" error — not a code panic.
        assert!(!result.is_empty(), "push must return some output");
    }

    #[tokio::test]
    async fn git_status_in_non_git_dir_returns_error_output() {
        use tempfile::TempDir;
        let dir = TempDir::new().unwrap();
        let non_git_ctx = ToolContext {
            proxy_url: String::new(),
            cwd: dir.path().to_string_lossy().into_owned(),
            http_client: reqwest::Client::new(),
            web_search_api_key: None,
            web_search_endpoint: None,
        };
        let result = status(&non_git_ctx, &json!({})).await;
        assert!(!result.is_empty());
    }
}

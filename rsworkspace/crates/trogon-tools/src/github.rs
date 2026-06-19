//! GitHub CLI tool — runs the host `gh` binary in the session working directory.
//!
//! This works the same way the Claude Code CLI does: instead of bespoke API
//! tools, the model drives `gh` directly. Because the generic `bash` tool is
//! WASM-sandboxed and cannot spawn host binaries, this shells out to the real
//! `gh` the same way the `git_*` tools shell out to real `git`.
//!
//! Running `gh` in `ctx.cwd` gives the Claude Code behavior for free:
//! - the repository is auto-detected from the git remote (no owner/repo args),
//! - authentication comes from the local `gh` login (or `GH_TOKEN`/`GITHUB_TOKEN`),
//! - the full surface is available: PRs, issues, checks, releases, and `gh api`.

use serde_json::Value;

use crate::ToolContext;

/// Cap on captured `gh` output. Larger than the `git_*` tools' 4KB cap because
/// `gh pr diff` / `gh api` returns content the model actually needs, but still
/// bounded so a huge response can't exhaust memory or exceed the NATS payload
/// limit. One byte past the cap is read so truncation can be detected and flagged.
const MAX_OUTPUT: usize = 64 * 1024;
/// Hard cap on bytes buffered in memory before the child is killed.
const MAX_STDOUT_BUFFER: usize = 512 * 1024;

/// Build the argv for `gh` from the tool input.
///
/// Accepts either `args` (an array passed through verbatim — no shell parsing)
/// or `command` (a single string split with POSIX shell quoting via shlex). An
/// optional leading `gh` token is stripped so both `pr list` and `gh pr list`
/// work.
fn parse_argv(input: &Value) -> Result<Vec<String>, String> {
    let mut argv: Vec<String> = if let Some(arr) = input.get("args").and_then(|v| v.as_array()) {
        arr.iter()
            .map(|v| v.as_str().map(str::to_string))
            .collect::<Option<Vec<_>>>()
            .ok_or("`args` must be an array of strings")?
    } else if let Some(cmd) = input.get("command").and_then(|v| v.as_str()) {
        shlex::split(cmd).ok_or("could not parse `command` (unbalanced quotes?)")?
    } else {
        return Err("provide `command` (a string) or `args` (an array of strings)".to_string());
    };

    if argv.first().map(String::as_str) == Some("gh") {
        argv.remove(0);
    }
    if argv.is_empty() {
        return Err("no gh arguments provided".to_string());
    }
    if argv.iter().any(|a| a.contains('\0')) {
        return Err("arguments must not contain NUL bytes".to_string());
    }
    Ok(argv)
}

/// Run the GitHub CLI in the session working directory.
pub async fn gh(ctx: &ToolContext, input: &Value) -> String {
    let argv = match parse_argv(input) {
        Ok(a) => a,
        Err(e) => return format!("Error: {e}"),
    };
    run_gh(&ctx.cwd, &argv).await
}

async fn run_gh(cwd: &str, args: &[String]) -> String {
    use tokio::io::AsyncReadExt;
    use tokio::process::Command;

    // stdin is closed so `gh` never blocks on an interactive prompt; with no TTY
    // it errors out fast instead of hanging.
    let mut child = match Command::new("gh")
        .args(args)
        .current_dir(cwd)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
    {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return "Error: `gh` CLI not found. Install GitHub CLI (https://cli.github.com) \
                    and run `gh auth login`."
                .to_string();
        }
        Err(e) => return format!("Error running gh: {e}"),
    };

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
        format!("{}... (truncated at 64KB)", &combined[..boundary])
    } else {
        combined
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parse_argv_from_args_array() {
        let argv = parse_argv(&json!({"args": ["pr", "view", "42"]})).unwrap();
        assert_eq!(argv, vec!["pr", "view", "42"]);
    }

    #[test]
    fn parse_argv_from_command_string_respects_quotes() {
        let argv = parse_argv(&json!({
            "command": "pr create --title \"My PR\" --body 'hello world'"
        }))
        .unwrap();
        assert_eq!(argv, vec!["pr", "create", "--title", "My PR", "--body", "hello world"]);
    }

    #[test]
    fn parse_argv_strips_leading_gh() {
        let argv = parse_argv(&json!({"command": "gh issue list"})).unwrap();
        assert_eq!(argv, vec!["issue", "list"]);
        let argv = parse_argv(&json!({"args": ["gh", "issue", "list"]})).unwrap();
        assert_eq!(argv, vec!["issue", "list"]);
    }

    #[test]
    fn parse_argv_missing_input_errors() {
        let err = parse_argv(&json!({})).unwrap_err();
        assert!(err.contains("command") && err.contains("args"), "got: {err}");
    }

    #[test]
    fn parse_argv_empty_after_strip_errors() {
        let err = parse_argv(&json!({"command": "gh"})).unwrap_err();
        assert!(err.contains("no gh arguments"), "got: {err}");
    }

    #[test]
    fn parse_argv_unbalanced_quotes_errors() {
        let err = parse_argv(&json!({"command": "pr create --title \"unterminated"})).unwrap_err();
        assert!(err.contains("parse"), "got: {err}");
    }

    #[test]
    fn parse_argv_non_string_args_error() {
        let err = parse_argv(&json!({"args": ["pr", 42]})).unwrap_err();
        assert!(err.contains("array of strings"), "got: {err}");
    }

    #[tokio::test]
    async fn gh_missing_input_returns_error() {
        let ctx = ToolContext {
            proxy_url: String::new(),
            cwd: ".".to_string(),
            http_client: reqwest::Client::new(),
            web_search_api_key: None,
            web_search_endpoint: None,
        };
        let out = gh(&ctx, &json!({})).await;
        assert!(out.starts_with("Error:"), "got: {out}");
    }

    // Live: requires `gh` installed. Runs `gh --version` in the cwd.
    #[tokio::test]
    #[ignore]
    async fn gh_version_live() {
        let out = run_gh(".", &["--version".to_string()]).await;
        eprintln!("gh --version => {out}");
        assert!(out.contains("gh version") || out.contains("not found"), "got: {out}");
    }
}

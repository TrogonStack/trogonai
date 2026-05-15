use serde_json::Value;

use crate::tools::ToolContext;

const MAX_OUTPUT: usize = 4 * 1024;

async fn run_git(cwd: &str, args: &[&str]) -> String {
    let output = tokio::process::Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .await;

    match output {
        Ok(o) => {
            let stdout = String::from_utf8_lossy(&o.stdout);
            let stderr = String::from_utf8_lossy(&o.stderr);
            let combined = if stderr.is_empty() {
                stdout.into_owned()
            } else if stdout.is_empty() {
                stderr.into_owned()
            } else {
                format!("{stdout}{stderr}")
            };
            if combined.is_empty() {
                "(no output)".to_string()
            } else if combined.len() > MAX_OUTPUT {
                format!("{}... (truncated at 4KB)", &combined[..MAX_OUTPUT])
            } else {
                combined
            }
        }
        Err(e) => format!("Error running git: {e}"),
    }
}

pub async fn status(ctx: &ToolContext, _input: &Value) -> String {
    run_git(&ctx.cwd, &["status", "--short"]).await
}

pub async fn diff(ctx: &ToolContext, input: &Value) -> String {
    let mut args = vec!["diff"];
    let extra = input.get("args").and_then(|v| v.as_str());
    if let Some(e) = extra {
        args.push(e);
    }
    run_git(&ctx.cwd, &args).await
}

pub async fn log(ctx: &ToolContext, _input: &Value) -> String {
    run_git(&ctx.cwd, &["log", "--oneline", "-20"]).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn ctx() -> ToolContext {
        ToolContext {
            proxy_url: String::new(),
            cwd: std::env::current_dir()
                .unwrap()
                .to_string_lossy()
                .into_owned(),
            http_client: reqwest::Client::new(),
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
        };
        let result = diff(&large_ctx, &json!({"args": "--staged"})).await;
        assert!(result.contains("truncated at 4KB"), "expected truncation, got: {result}");
    }

    #[tokio::test]
    async fn git_status_in_non_git_dir_returns_error_output() {
        use tempfile::TempDir;
        let dir = TempDir::new().unwrap();
        let non_git_ctx = ToolContext {
            proxy_url: String::new(),
            cwd: dir.path().to_string_lossy().into_owned(),
            http_client: reqwest::Client::new(),
        };
        let result = status(&non_git_ctx, &json!({})).await;
        assert!(!result.is_empty());
    }
}

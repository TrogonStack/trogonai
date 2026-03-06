//! Handler: CI check run completed.
//!
//! Triggered by a `check_run` webhook event with `action: "completed"`.
//! On failure the agent analyses the logs and posts a diagnostic comment
//! on the related PR.  On success it does nothing.

use serde_json::Value;
use tracing::{info, warn};

use crate::agent_loop::AgentLoop;
use crate::tools::{ToolDef, tool_def};
use super::{fetch_memory, run_agent, DEFAULT_MEMORY_PATH};

/// Run the CI-completed agent from a raw GitHub `check_run` webhook payload.
///
/// Returns `None` when the action is not `"completed"` or the conclusion is
/// `"success"` / `"skipped"`.
pub async fn handle(agent: &AgentLoop, payload: &[u8]) -> Option<Result<String, String>> {
    let event: Value = match serde_json::from_slice(payload) {
        Ok(v) => v,
        Err(e) => return Some(Err(format!("JSON parse error: {e}"))),
    };

    if event["action"].as_str() != Some("completed") {
        return None;
    }

    let conclusion = event["check_run"]["conclusion"].as_str().unwrap_or("");
    // Only act on failures — success/skipped/neutral need no agent attention.
    if matches!(conclusion, "success" | "skipped" | "neutral") {
        return None;
    }

    let owner = event["repository"]["owner"]["login"].as_str()?;
    let repo = event["repository"]["name"].as_str()?;
    let check_name = event["check_run"]["name"].as_str().unwrap_or("CI");
    let details_url = event["check_run"]["details_url"].as_str().unwrap_or("");

    // Extract PR number from the check_run's pull_requests array if present.
    let pr_number = event["check_run"]["pull_requests"]
        .as_array()
        .and_then(|prs| prs.first())
        .and_then(|pr| pr["number"].as_u64());

    info!(owner, repo, check_name, conclusion, "Starting CI-completed agent");

    let pr_context = match pr_number {
        Some(n) => format!("This check is associated with PR #{n}."),
        None => "This check is not directly associated with a PR.".to_string(),
    };

    let prompt = format!(
        "CI check `{check_name}` in {owner}/{repo} completed with conclusion: `{conclusion}`.\n\
         {pr_context}\n\
         Details: {details_url}\n\n\
         1. Use `get_file_contents` to inspect relevant config or test files if helpful.\n\
         2. Analyse the likely cause of the failure based on the check name and context.\n\
         3. Post a diagnostic comment on the PR (if any) using `post_pr_comment`.\n\
         Be concise — one paragraph explaining the likely issue and suggested fix."
    );

    let mem_path = agent.memory_path.as_deref().unwrap_or(DEFAULT_MEMORY_PATH);
    let memory = fetch_memory(agent, owner, repo, mem_path).await;

    match run_agent(agent, prompt, ci_tools(), memory).await {
        Ok(text) => {
            info!(check_name, "CI-completed agent done");
            Some(Ok(text))
        }
        Err(e) => {
            warn!(check_name, error = %e, "CI-completed agent failed");
            Some(Err(e.to_string()))
        }
    }
}

fn ci_tools() -> Vec<ToolDef> {
    vec![
        tool_def("get_file_contents",
            "Read a file from the repository. Returns JSON with `sha` and `content`.",
            serde_json::json!({
                "type": "object", "required": ["owner","repo","path"],
                "properties": {
                    "owner": {"type":"string"}, "repo": {"type":"string"},
                    "path": {"type":"string"}, "ref": {"type":"string"}
                }
            }),
        ),
        tool_def("post_pr_comment", "Post a comment on a pull request.",
            serde_json::json!({
                "type": "object", "required": ["owner","repo","pr_number","body"],
                "properties": {
                    "owner": {"type":"string"}, "repo": {"type":"string"},
                    "pr_number": {"type":"integer"}, "body": {"type":"string"}
                }
            }),
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ci_tools_has_two_entries() {
        assert_eq!(ci_tools().len(), 2);
    }

    #[tokio::test]
    async fn handle_skips_non_completed_action() {
        use std::sync::Arc;
        use crate::agent_loop::AgentLoop;
        use crate::tools::ToolContext;
        let agent = AgentLoop {
            http_client: reqwest::Client::new(),
            proxy_url: "http://localhost:9999".to_string(),
            anthropic_token: String::new(),
            model: "test".to_string(),
            max_iterations: 1,
            tool_context: Arc::new(ToolContext {
                http_client: reqwest::Client::new(),
                proxy_url: "http://localhost:9999".to_string(),
                github_token: String::new(),
                linear_token: String::new(),
            }),
            memory_owner: None,
            memory_repo: None,
            memory_path: None,
            mcp_tool_defs: vec![],
            mcp_dispatch: vec![],
        };
        let payload = serde_json::json!({
            "action": "created",
            "check_run": {"name": "CI", "conclusion": null, "pull_requests": [], "details_url": ""},
            "repository": {"owner": {"login": "o"}, "name": "r"}
        });
        assert!(handle(&agent, &serde_json::to_vec(&payload).unwrap()).await.is_none());
    }

    #[tokio::test]
    async fn handle_skips_successful_run() {
        use std::sync::Arc;
        use crate::agent_loop::AgentLoop;
        use crate::tools::ToolContext;
        let agent = AgentLoop {
            http_client: reqwest::Client::new(),
            proxy_url: "http://localhost:9999".to_string(),
            anthropic_token: String::new(),
            model: "test".to_string(),
            max_iterations: 1,
            tool_context: Arc::new(ToolContext {
                http_client: reqwest::Client::new(),
                proxy_url: "http://localhost:9999".to_string(),
                github_token: String::new(),
                linear_token: String::new(),
            }),
            memory_owner: None,
            memory_repo: None,
            memory_path: None,
            mcp_tool_defs: vec![],
            mcp_dispatch: vec![],
        };
        let payload = serde_json::json!({
            "action": "completed",
            "check_run": {"name": "CI", "conclusion": "success", "pull_requests": [], "details_url": ""},
            "repository": {"owner": {"login": "o"}, "name": "r"}
        });
        assert!(handle(&agent, &serde_json::to_vec(&payload).unwrap()).await.is_none());
    }
}

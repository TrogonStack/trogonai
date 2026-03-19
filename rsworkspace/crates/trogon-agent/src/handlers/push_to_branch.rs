//! Handler: new push to branch.
//!
//! Triggered by a `push` webhook event.  The agent analyses the commits
//! and posts a summary comment on any open PR targeting that branch.

use serde_json::Value;
use tracing::{info, warn};

use super::{DEFAULT_MEMORY_PATH, fetch_memory, run_agent};
use crate::agent_loop::AgentLoop;
use crate::tools::{ToolDef, slack, tool_def};

/// Run the push-to-branch agent from a raw GitHub `push` webhook payload.
///
/// Returns `None` for deletion events (empty `after`) or tag pushes.
pub async fn handle(agent: &AgentLoop, payload: &[u8]) -> Option<Result<String, String>> {
    let event: Value = match serde_json::from_slice(payload) {
        Ok(v) => v,
        Err(e) => return Some(Err(format!("JSON parse error: {e}"))),
    };

    // Skip tag pushes and deletions.
    let git_ref = event["ref"].as_str().unwrap_or("");
    if !git_ref.starts_with("refs/heads/") {
        return None;
    }
    if event["deleted"].as_bool() == Some(true) {
        return None;
    }

    let owner = event["repository"]["owner"]["login"].as_str()?;
    let repo = event["repository"]["name"].as_str()?;
    let branch = git_ref.trim_start_matches("refs/heads/");
    let pusher = event["pusher"]["name"].as_str().unwrap_or("unknown");
    let commit_count = event["commits"].as_array().map(|c| c.len()).unwrap_or(0);
    let head_commit_msg = event["head_commit"]["message"]
        .as_str()
        .unwrap_or("(no message)");

    info!(
        owner,
        repo, branch, pusher, commit_count, "Starting push-to-branch agent"
    );

    let prompt = format!(
        "{pusher} pushed {commit_count} commit(s) to branch `{branch}` in {owner}/{repo}.\n\
         Head commit: \"{head_commit_msg}\"\n\n\
         1. Use `get_file_contents` to inspect any changed files that look risky \
            (e.g. config, CI, dependencies).\n\
         2. If anything looks problematic, post a comment on an open PR for this branch \
            using `post_pr_comment`.\n\
         3. Otherwise, do nothing — not every push needs a comment.\n\
         Be concise and only act when genuinely needed."
    );

    let mem_path = agent.memory_path.as_deref().unwrap_or(DEFAULT_MEMORY_PATH);
    let memory = fetch_memory(agent, owner, repo, mem_path).await;

    match run_agent(agent, prompt, push_tools(), memory).await {
        Ok(text) => {
            info!(branch, "Push-to-branch agent completed");
            Some(Ok(text))
        }
        Err(e) => {
            warn!(branch, error = %e, "Push-to-branch agent failed");
            Some(Err(e.to_string()))
        }
    }
}

fn push_tools() -> Vec<ToolDef> {
    let mut tools = vec![
        tool_def(
            "get_file_contents",
            "Read a file from the repository. Returns JSON with `sha` and `content`.",
            serde_json::json!({
                "type": "object", "required": ["owner","repo","path"],
                "properties": {
                    "owner": {"type":"string"}, "repo": {"type":"string"},
                    "path": {"type":"string"}, "ref": {"type":"string"}
                }
            }),
        ),
        tool_def(
            "post_pr_comment",
            "Post a comment on a pull request.",
            serde_json::json!({
                "type": "object", "required": ["owner","repo","pr_number","body"],
                "properties": {
                    "owner": {"type":"string"}, "repo": {"type":"string"},
                    "pr_number": {"type":"integer"}, "body": {"type":"string"}
                }
            }),
        ),
    ];
    tools.extend(slack::slack_tool_defs());
    tools
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn push_tools_has_two_entries() {
        assert_eq!(push_tools().len(), 4);
    }

    #[tokio::test]
    async fn handle_skips_tag_push() {
        use crate::agent_loop::AgentLoop;
        use crate::tools::ToolContext;
        use std::sync::Arc;
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
                slack_token: String::new(),
            }),
            memory_owner: None,
            memory_repo: None,
            memory_path: None,
            mcp_tool_defs: vec![],
            mcp_dispatch: vec![],
            split_client: None,
            tenant_id: "test".to_string(),
            anthropic_base_url: None,
            anthropic_extra_headers: vec![],
            thinking_budget: None,
            permission_checker: None,
        };
        let payload = serde_json::json!({
            "ref": "refs/tags/v1.0.0",
            "deleted": false,
            "pusher": {"name": "u"},
            "commits": [],
            "head_commit": {"message": "tag"},
            "repository": {"owner": {"login": "o"}, "name": "r"}
        });
        assert!(
            handle(&agent, &serde_json::to_vec(&payload).unwrap())
                .await
                .is_none()
        );
    }

    #[tokio::test]
    async fn handle_skips_deletion_event() {
        use crate::agent_loop::AgentLoop;
        use crate::tools::ToolContext;
        use std::sync::Arc;
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
                slack_token: String::new(),
            }),
            memory_owner: None,
            memory_repo: None,
            memory_path: None,
            mcp_tool_defs: vec![],
            mcp_dispatch: vec![],
            split_client: None,
            tenant_id: "test".to_string(),
            anthropic_base_url: None,
            anthropic_extra_headers: vec![],
            thinking_budget: None,
            permission_checker: None,
        };
        let payload = serde_json::json!({
            "ref": "refs/heads/feature-x",
            "deleted": true,
            "pusher": {"name": "u"},
            "commits": [],
            "head_commit": null,
            "repository": {"owner": {"login": "o"}, "name": "r"}
        });
        assert!(
            handle(&agent, &serde_json::to_vec(&payload).unwrap())
                .await
                .is_none()
        );
    }
}

//! Handler: PR merged.
//!
//! Triggered when a `pull_request` webhook arrives with `action: "closed"`
//! and `pull_request.merged: true`.  The agent summarises what was merged,
//! optionally updates `.trogon/memory.md` with lessons learned, and posts a
//! brief merge-acknowledgement comment on the PR.

use serde_json::Value;
use tracing::{info, warn};

use super::{DEFAULT_MEMORY_PATH, fetch_memory, run_agent};
use crate::agent_loop::AgentLoop;
use crate::tools::{ToolDef, slack, tool_def};

/// Run the PR-merged agent from a raw GitHub `pull_request` webhook payload.
///
/// Returns `None` when the PR was closed without merging.
pub async fn handle(agent: &AgentLoop, payload: &[u8]) -> Option<Result<String, String>> {
    let event: Value = match serde_json::from_slice(payload) {
        Ok(v) => v,
        Err(e) => return Some(Err(format!("JSON parse error: {e}"))),
    };

    if event["action"].as_str() != Some("closed") {
        return None;
    }
    if event["pull_request"]["merged"].as_bool() != Some(true) {
        return None;
    }

    let owner = event["repository"]["owner"]["login"].as_str()?;
    let repo = event["repository"]["name"].as_str()?;
    let pr_number = event["number"].as_u64()?;
    let title = event["pull_request"]["title"]
        .as_str()
        .unwrap_or("(no title)");
    let merged_by = event["pull_request"]["merged_by"]["login"]
        .as_str()
        .unwrap_or("unknown");

    info!(owner, repo, pr_number, "Starting PR-merged agent");

    let prompt = format!(
        "Pull request #{pr_number} \"{title}\" in {owner}/{repo} was just merged by {merged_by}.\n\
         1. Use `get_pr_diff` to read what changed.\n\
         2. Use `get_pr_comments` to recall the review discussion.\n\
         3. Post a brief merge-acknowledgement comment using `post_pr_comment` \
            (e.g. key decisions, anything to watch).\n\
         4. If you learned something important about the repo conventions, \
            update `.trogon/memory.md` using `update_file` (pass the sha from \
            `get_file_contents`), then open a PR with `create_pull_request`.\n\
         Be concise."
    );

    let mem_path = agent.memory_path.as_deref().unwrap_or(DEFAULT_MEMORY_PATH);
    let memory = fetch_memory(agent, owner, repo, mem_path).await;

    match run_agent(agent, prompt, merged_tools(), memory).await {
        Ok(text) => {
            info!(pr_number, "PR-merged agent completed");
            Some(Ok(text))
        }
        Err(e) => {
            warn!(pr_number, error = %e, "PR-merged agent failed");
            Some(Err(e.to_string()))
        }
    }
}

fn merged_tools() -> Vec<ToolDef> {
    let mut tools = vec![
        tool_def(
            "get_pr_diff",
            "Get the unified diff of a pull request.",
            serde_json::json!({
                "type": "object", "required": ["owner","repo","pr_number"],
                "properties": {
                    "owner": {"type":"string"}, "repo": {"type":"string"},
                    "pr_number": {"type":"integer"}
                }
            }),
        ),
        tool_def(
            "get_pr_comments",
            "Get all comments on a pull request.",
            serde_json::json!({
                "type": "object", "required": ["owner","repo","pr_number"],
                "properties": {
                    "owner": {"type":"string"}, "repo": {"type":"string"},
                    "pr_number": {"type":"integer"}
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
                    "pr_number": {"type":"integer"},
                    "body": {"type":"string", "description":"Markdown comment body"}
                }
            }),
        ),
        tool_def(
            "get_file_contents",
            "Read a file. Returns JSON with `sha` and `content`.",
            serde_json::json!({
                "type": "object", "required": ["owner","repo","path"],
                "properties": {
                    "owner": {"type":"string"}, "repo": {"type":"string"},
                    "path": {"type":"string"}, "ref": {"type":"string"}
                }
            }),
        ),
        tool_def(
            "update_file",
            "Create or update a file in the repository.",
            serde_json::json!({
                "type": "object", "required": ["owner","repo","path","message","content"],
                "properties": {
                    "owner": {"type":"string"}, "repo": {"type":"string"},
                    "path": {"type":"string"}, "message": {"type":"string"},
                    "content": {"type":"string"}, "branch": {"type":"string"},
                    "sha": {"type":"string"}
                }
            }),
        ),
        tool_def(
            "create_pull_request",
            "Open a pull request.",
            serde_json::json!({
                "type": "object", "required": ["owner","repo","title","head"],
                "properties": {
                    "owner": {"type":"string"}, "repo": {"type":"string"},
                    "title": {"type":"string"}, "head": {"type":"string"},
                    "base": {"type":"string"}, "body": {"type":"string"}
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
    fn merged_tools_has_six_entries() {
        assert_eq!(merged_tools().len(), 8);
    }

    #[tokio::test]
    async fn handle_skips_non_closed_action() {
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
            "action": "opened",
            "number": 1,
            "pull_request": {"merged": true, "title": "t", "merged_by": {"login": "u"}},
            "repository": {"owner": {"login": "o"}, "name": "r"}
        });
        assert!(
            handle(&agent, &serde_json::to_vec(&payload).unwrap())
                .await
                .is_none()
        );
    }

    #[tokio::test]
    async fn handle_skips_closed_but_not_merged() {
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
            "action": "closed",
            "number": 1,
            "pull_request": {"merged": false, "title": "t", "merged_by": null},
            "repository": {"owner": {"login": "o"}, "name": "r"}
        });
        assert!(
            handle(&agent, &serde_json::to_vec(&payload).unwrap())
                .await
                .is_none()
        );
    }

    #[tokio::test]
    async fn handle_returns_error_on_invalid_json() {
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
        assert!(matches!(handle(&agent, b"not json").await, Some(Err(_))));
    }
}

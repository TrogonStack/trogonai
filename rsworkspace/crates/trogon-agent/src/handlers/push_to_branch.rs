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

    fn make_stub_agent() -> AgentLoop {
        use crate::agent_loop::mock::SequencedMockAnthropicClient;
        use crate::flag_client::AlwaysOnFlagClient;
        use crate::tools::{DefaultToolDispatcher, ToolContext};
        use std::sync::Arc;
        let tool_ctx = Arc::new(ToolContext::for_test("http://localhost:9999", "", "", ""));
        AgentLoop {
            anthropic_client: Arc::new(SequencedMockAnthropicClient::new(vec![])),
            model: "test".to_string(),
            max_iterations: 1,
            tool_dispatcher: Arc::new(DefaultToolDispatcher::new(Arc::clone(&tool_ctx))),
            tool_context: tool_ctx,
            memory_owner: None,
            memory_repo: None,
            memory_path: None,
            mcp_tool_defs: vec![],
            mcp_dispatch: vec![],
            flag_client: Arc::new(AlwaysOnFlagClient),
            tenant_id: "test".to_string(),
            promise_store: None,
            promise_id: None,
            permission_checker: None,
            elicitation_provider: None,
        }
    }

    #[tokio::test]
    async fn handle_skips_tag_push() {
        let payload = serde_json::json!({
            "ref": "refs/tags/v1.0.0",
            "deleted": false,
            "pusher": {"name": "u"},
            "commits": [],
            "head_commit": {"message": "tag"},
            "repository": {"owner": {"login": "o"}, "name": "r"}
        });
        assert!(
            handle(&make_stub_agent(), &serde_json::to_vec(&payload).unwrap())
                .await
                .is_none()
        );
    }

    #[tokio::test]
    async fn handle_skips_deletion_event() {
        let payload = serde_json::json!({
            "ref": "refs/heads/feature-x",
            "deleted": true,
            "pusher": {"name": "u"},
            "commits": [],
            "head_commit": null,
            "repository": {"owner": {"login": "o"}, "name": "r"}
        });
        assert!(
            handle(&make_stub_agent(), &serde_json::to_vec(&payload).unwrap())
                .await
                .is_none()
        );
    }

    #[tokio::test]
    async fn handle_returns_error_on_invalid_json() {
        let result = handle(&make_stub_agent(), b"not valid json{{").await;
        assert!(
            matches!(result, Some(Err(ref e)) if e.contains("JSON parse error")),
            "invalid JSON must return Some(Err(..)); got: {result:?}"
        );
    }

    /// When `repository.owner.login` or `repository.name` is absent the `?`
    /// operator on `as_str()` returns `None` — the handler skips silently.
    #[tokio::test]
    async fn handle_skips_when_repository_fields_absent() {
        // `refs/heads/` prefix passes the tag/deletion guard, but `owner` is missing.
        let payload = serde_json::json!({
            "ref": "refs/heads/main",
            "deleted": false,
            "pusher": {"name": "u"},
            "commits": [],
            "head_commit": {"message": "fix: typo"},
            "repository": {}   // no owner or name
        });
        let result = handle(&make_stub_agent(), &serde_json::to_vec(&payload).unwrap()).await;
        assert!(
            result.is_none(),
            "missing repository fields must cause the handler to return None; got: {result:?}"
        );
    }
}

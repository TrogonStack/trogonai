//! Handler: automated PR review.
//!
//! Triggered by a NATS message on `github.pull_request` (created / reopened /
//! synchronize actions).  The agent fetches the diff and changed files, then
//! posts a review comment back to the PR.
//!
//! # NATS payload
//! The message body must be a JSON object with at minimum:
//! ```json
//! {
//!   "action": "opened",
//!   "number": 42,
//!   "repository": { "owner": { "login": "org" }, "name": "repo" }
//! }
//! ```
//! This is the standard GitHub `pull_request` webhook payload shape.
//!
//! # Token requirements
//!
//! The GitHub proxy token must have **`pull_requests: write`** scope so that
//! `post_pr_review` can submit inline review comments via
//! `POST /repos/{owner}/{repo}/pulls/{number}/reviews`.
//! The existing `issues: write` scope (used by `post_pr_comment`) is not
//! sufficient for the reviews endpoint.

use serde_json::Value;
use tracing::{info, warn};

use super::{fetch_memory, run_agent};
use crate::agent_loop::AgentLoop;
use crate::promise_store::{AgentPromise, PromiseStatus};
use crate::tools::{ToolDef, slack, tool_def};

/// Actions that trigger a review.
const REVIEW_ACTIONS: &[&str] = &["opened", "reopened", "synchronize"];

/// Run the PR-review agent from a raw GitHub `pull_request` webhook payload.
///
/// Returns `None` when the action is not relevant (e.g. "closed", "labeled").
pub async fn handle(agent: &AgentLoop, payload: &[u8]) -> Option<Result<String, String>> {
    let event: Value = match serde_json::from_slice(payload) {
        Ok(v) => v,
        Err(e) => return Some(Err(format!("JSON parse error: {e}"))),
    };

    let action = event["action"].as_str().unwrap_or("");
    if !REVIEW_ACTIONS.contains(&action) {
        info!(action, "PR action not relevant for review — skipping");
        return None;
    }

    if event["pull_request"]["draft"].as_bool().unwrap_or(false) {
        info!("PR is a draft — skipping review");
        return None;
    }

    let owner = event["repository"]["owner"]["login"].as_str()?;
    let repo = event["repository"]["name"].as_str()?;
    let pr_number = event["number"].as_u64()?;
    let head_sha = event["pull_request"]["head"]["sha"]
        .as_str()
        .unwrap_or("");

    // SHA-based dedup: if rapid force-pushes fire multiple synchronize events
    // for the same commit, only the first one should trigger a review.
    if !head_sha.is_empty() {
        if let Some(store) = &agent.promise_store {
            let dedup_id = format!("pr-review-sha.{owner}.{repo}.{head_sha}");
            if store
                .get_promise(&agent.tenant_id, &dedup_id)
                .await
                .ok()
                .flatten()
                .is_some()
            {
                info!(head_sha, "PR head SHA already reviewed — skipping duplicate");
                return None;
            }
            let marker = AgentPromise {
                id: dedup_id,
                tenant_id: agent.tenant_id.clone(),
                automation_id: String::new(),
                status: PromiseStatus::Resolved,
                messages: vec![],
                iteration: 0,
                worker_id: String::new(),
                claimed_at: std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs(),
                trigger: serde_json::Value::Null,
                nats_subject: String::new(),
                system_prompt: None,
                recovery_count: 0,
                checkpoint_degraded: false,
                failure_reason: None,
            };
            let _ = store.put_promise(&marker).await;
        }
    }

    info!(owner, repo, pr_number, "Starting PR review agent");

    let prompt = format!(
        "You are a code reviewer. Your task is to review pull request #{pr_number} in \
         {owner}/{repo} and post the review using `post_pr_review`.\n\
         The head commit SHA is `{head_sha}`.\n\n\
         Steps:\n\
         1. Call `get_pr_comments` to see prior review discussion.\n\
         2. Call `list_pr_files` to see changed files. Each file with a `patch` field has its \
            diff lines prefixed with a 1-based position number — use those as the `position` \
            value in inline comments.\n\
         3. Optionally call `get_file_contents` for more context.\n\
         4. Call `post_pr_review` with your findings — this is REQUIRED. Set `commit_sha` to \
            `{head_sha}`. Set `event` to \"COMMENT\", \"APPROVE\", or \"REQUEST_CHANGES\". \
            Include inline `comments` (each with `path`, `position`, `body`) for every issue found.\n\n\
         Do NOT skip step 4. Do NOT just write a text summary — you MUST call `post_pr_review`.\n\
         Focus on bugs, security issues, and correctness."
    );

    let tools = pr_review_tools();

    // Pre-fetch memory file and inject as Anthropic system prompt.
    // Returns None gracefully when the file doesn't exist yet.
    let mem_path = agent
        .memory_path
        .as_deref()
        .unwrap_or(super::DEFAULT_MEMORY_PATH);
    let memory = fetch_memory(agent, owner, repo, mem_path).await;

    match run_agent(agent, prompt, tools, memory).await {
        Ok(text) => {
            info!(pr_number, "PR review completed");
            Some(Ok(text))
        }
        Err(e) => {
            warn!(pr_number, error = %e, "PR review agent failed");
            Some(Err(e.to_string()))
        }
    }
}

fn pr_review_tools() -> Vec<ToolDef> {
    let mut tools = vec![
        tool_def(
            "list_pr_files",
            "List the files changed in a pull request.",
            serde_json::json!({
                "type": "object",
                "required": ["owner", "repo", "pr_number"],
                "properties": {
                    "owner":     { "type": "string", "description": "Repository owner" },
                    "repo":      { "type": "string", "description": "Repository name" },
                    "pr_number": { "type": "integer", "description": "Pull request number" }
                }
            }),
        ),
        tool_def(
            "get_pr_diff",
            "Get the unified diff of a pull request.",
            serde_json::json!({
                "type": "object",
                "required": ["owner", "repo", "pr_number"],
                "properties": {
                    "owner":     { "type": "string" },
                    "repo":      { "type": "string" },
                    "pr_number": { "type": "integer" }
                }
            }),
        ),
        tool_def(
            "get_file_contents",
            "Read the contents of a file at a specific git ref. Returns a JSON object with `sha` (blob SHA — required for update_file on existing files) and `content` (decoded UTF-8 text).",
            serde_json::json!({
                "type": "object",
                "required": ["owner", "repo", "path"],
                "properties": {
                    "owner": { "type": "string" },
                    "repo":  { "type": "string" },
                    "path":  { "type": "string", "description": "File path in the repository" },
                    "ref":   { "type": "string", "description": "Git ref (branch, tag, SHA). Defaults to HEAD." }
                }
            }),
        ),
        tool_def(
            "get_pr_comments",
            "Get all comments on a pull request — use this to recall prior decisions and context.",
            serde_json::json!({
                "type": "object",
                "required": ["owner", "repo", "pr_number"],
                "properties": {
                    "owner":     { "type": "string" },
                    "repo":      { "type": "string" },
                    "pr_number": { "type": "integer" }
                }
            }),
        ),
        tool_def(
            "post_pr_review",
            "Post a pull request review with optional inline diff comments. Prefer this over `post_pr_comment` for code review feedback.",
            serde_json::json!({
                "type": "object",
                "required": ["owner", "repo", "pr_number", "body", "event"],
                "properties": {
                    "owner":      { "type": "string" },
                    "repo":       { "type": "string" },
                    "pr_number":  { "type": "integer" },
                    "commit_sha": { "type": "string", "description": "PR head commit SHA — required when submitting inline comments" },
                    "body":       { "type": "string", "description": "Overall review summary" },
                    "event":      {
                        "type": "string",
                        "enum": ["COMMENT", "APPROVE", "REQUEST_CHANGES"],
                        "description": "Review disposition"
                    },
                    "comments": {
                        "type": "array",
                        "description": "Inline comments on specific diff lines",
                        "items": {
                            "type": "object",
                            "required": ["path", "position", "body"],
                            "properties": {
                                "path":     { "type": "string", "description": "File path relative to repo root" },
                                "position": { "type": "integer", "description": "1-based line position in the file's annotated diff patch" },
                                "body":     { "type": "string", "description": "Comment text (Markdown)" }
                            }
                        }
                    }
                }
            }),
        ),
        tool_def(
            "update_file",
            "Create or update a file in the repository. Use this to update .trogon/memory.md with learned context.",
            serde_json::json!({
                "type": "object",
                "required": ["owner", "repo", "path", "message", "content"],
                "properties": {
                    "owner":   { "type": "string" },
                    "repo":    { "type": "string" },
                    "path":    { "type": "string", "description": "File path (e.g. .trogon/memory.md)" },
                    "message": { "type": "string", "description": "Commit message" },
                    "content": { "type": "string", "description": "Full file content (plain UTF-8)" },
                    "branch":  { "type": "string", "description": "Target branch. Defaults to main." },
                    "sha":     { "type": "string", "description": "Current blob SHA — required when updating an existing file." }
                }
            }),
        ),
        tool_def(
            "create_pull_request",
            "Open a pull request. Use this to propose changes such as memory file updates.",
            serde_json::json!({
                "type": "object",
                "required": ["owner", "repo", "title", "head"],
                "properties": {
                    "owner": { "type": "string" },
                    "repo":  { "type": "string" },
                    "title": { "type": "string", "description": "PR title" },
                    "head":  { "type": "string", "description": "Branch containing the changes" },
                    "base":  { "type": "string", "description": "Target branch. Defaults to main." },
                    "body":  { "type": "string", "description": "PR description (Markdown)" }
                }
            }),
        ),
        tool_def(
            "request_reviewers",
            "Request reviewers on a pull request.",
            serde_json::json!({
                "type": "object",
                "required": ["owner", "repo", "pr_number", "reviewers"],
                "properties": {
                    "owner":     { "type": "string" },
                    "repo":      { "type": "string" },
                    "pr_number": { "type": "integer" },
                    "reviewers": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "GitHub usernames to request as reviewers"
                    }
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
    fn pr_review_tools_has_expected_count() {
        assert_eq!(pr_review_tools().len(), 10);
    }

    #[test]
    fn pr_review_tools_includes_expected_tools() {
        let tools = pr_review_tools();
        let names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
        assert!(names.contains(&"get_pr_comments"));
        assert!(names.contains(&"update_file"));
        assert!(names.contains(&"create_pull_request"));
        assert!(names.contains(&"post_pr_review"));
        // post_pr_comment was replaced by post_pr_review — must not be present.
        assert!(
            !names.contains(&"post_pr_comment"),
            "post_pr_comment must not be in pr_review tools"
        );
    }

    #[test]
    fn review_actions_contains_opened() {
        assert!(REVIEW_ACTIONS.contains(&"opened"));
        assert!(REVIEW_ACTIONS.contains(&"synchronize"));
        assert!(!REVIEW_ACTIONS.contains(&"closed"));
    }

    #[tokio::test]
    async fn handle_synchronize_action_is_not_skipped() {
        let payload = serde_json::json!({
            "action": "synchronize",
            "number": 20,
            "pull_request": { "draft": false, "head": { "sha": "syncsha" } },
            "repository": {"owner": {"login": "o"}, "name": "r"}
        });
        assert!(
            handle(
                &make_agent_with_responses(vec![end_turn()]),
                &serde_json::to_vec(&payload).unwrap(),
            )
            .await
            .is_some(),
            "synchronize must not be skipped"
        );
    }

    #[test]
    fn prompt_mentions_post_pr_review() {
        let owner = "o";
        let repo = "r";
        let pr_number = 1u64;
        let head_sha = "abc";
        let prompt = format!(
            "You are a code reviewer. Your task is to review pull request #{pr_number} in \
             {owner}/{repo} and post the review using `post_pr_review`.\n\
             The head commit SHA is `{head_sha}`.\n\n\
             Steps:\n\
             1. Call `get_pr_comments` to see prior review discussion.\n\
             2. Call `list_pr_files` to see changed files. Each file with a `patch` field has its \
                diff lines prefixed with a 1-based position number — use those as the `position` \
                value in inline comments.\n\
             3. Optionally call `get_file_contents` for more context.\n\
             4. Call `post_pr_review` with your findings — this is REQUIRED. Set `commit_sha` to \
                `{head_sha}`. Set `event` to \"COMMENT\", \"APPROVE\", or \"REQUEST_CHANGES\". \
                Include inline `comments` (each with `path`, `position`, `body`) for every issue found.\n\n\
             Do NOT skip step 4. Do NOT just write a text summary — you MUST call `post_pr_review`.\n\
             Focus on bugs, security issues, and correctness."
        );
        assert!(prompt.contains("post_pr_review"), "prompt must mention post_pr_review");
        assert!(prompt.contains("position"), "prompt must mention position for inline comments");
        assert!(
            prompt.contains("you MUST call `post_pr_review`"),
            "prompt must explicitly mandate calling the tool"
        );
        assert!(prompt.contains(head_sha), "prompt must embed head SHA");
    }

    fn make_agent_with_responses(responses: Vec<serde_json::Value>) -> AgentLoop {
        use crate::agent_loop::mock::SequencedMockAnthropicClient;
        use crate::flag_client::AlwaysOnFlagClient;
        use crate::tools::{DefaultToolDispatcher, ToolContext};
        use std::sync::Arc;
        let tool_ctx = Arc::new(ToolContext::for_test("http://localhost:9999", "", "", ""));
        AgentLoop {
            anthropic_client: Arc::new(SequencedMockAnthropicClient::new(responses)),
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

    fn make_agent() -> AgentLoop {
        make_agent_with_responses(vec![])
    }

    fn end_turn() -> serde_json::Value {
        serde_json::json!({"stop_reason": "end_turn", "content": [{"type": "text", "text": "ok"}]})
    }

    #[tokio::test]
    async fn handle_skips_non_review_action() {
        let payload = serde_json::json!({
            "action": "closed",
            "number": 5,
            "repository": {"owner": {"login": "o"}, "name": "r"}
        });
        assert!(
            handle(&make_agent(), &serde_json::to_vec(&payload).unwrap())
                .await
                .is_none()
        );
    }

    #[tokio::test]
    async fn handle_returns_error_on_invalid_json() {
        assert!(matches!(
            handle(&make_agent(), b"not json").await,
            Some(Err(_))
        ));
    }

    #[tokio::test]
    async fn handle_skips_draft_pr() {
        let payload = serde_json::json!({
            "action": "opened",
            "number": 3,
            "pull_request": { "draft": true, "head": { "sha": "abc" } },
            "repository": {"owner": {"login": "o"}, "name": "r"}
        });
        assert!(
            handle(&make_agent(), &serde_json::to_vec(&payload).unwrap())
                .await
                .is_none(),
            "draft PR must be skipped"
        );
    }

    #[tokio::test]
    async fn handle_skips_duplicate_sha() {
        use crate::promise_store::mock::MockPromiseStore;
        use crate::promise_store::{AgentPromise, PromiseStatus};
        use std::sync::Arc;

        let store = MockPromiseStore::new();
        store.insert_promise(AgentPromise {
            id: "pr-review-sha.o.r.deadbeef".to_string(),
            tenant_id: "test".to_string(),
            automation_id: String::new(),
            status: PromiseStatus::Resolved,
            messages: vec![],
            iteration: 0,
            worker_id: String::new(),
            claimed_at: 0,
            trigger: serde_json::Value::Null,
            nats_subject: String::new(),
            system_prompt: None,
            recovery_count: 0,
            checkpoint_degraded: false,
            failure_reason: None,
        });

        let mut agent = make_agent();
        agent.promise_store = Some(Arc::new(store));

        let payload = serde_json::json!({
            "action": "synchronize",
            "number": 4,
            "pull_request": { "draft": false, "head": { "sha": "deadbeef" } },
            "repository": {"owner": {"login": "o"}, "name": "r"}
        });
        assert!(
            handle(&agent, &serde_json::to_vec(&payload).unwrap())
                .await
                .is_none(),
            "duplicate SHA must be skipped"
        );
    }

    #[test]
    fn prompt_contains_head_sha() {
        // Verify the format string embeds the SHA — no agent run needed.
        let sha = "deadbeef1234";
        let prompt = format!(
            "head commit SHA is `{sha}` — pass it as `commit_sha`",
            sha = sha
        );
        assert!(prompt.contains(sha));
    }

    /// When `repository.owner.login`, `repository.name`, or `number` is absent
    /// the `?` operator on lines 43-45 returns `None` — the handler skips silently.
    #[tokio::test]
    async fn handle_skips_when_required_fields_absent() {
        // Action passes the guard but repository is empty.
        let payload = serde_json::json!({
            "action": "opened",
            "number": 7,
            "repository": {}   // no owner or name
        });
        assert!(
            handle(&make_agent(), &serde_json::to_vec(&payload).unwrap())
                .await
                .is_none(),
            "missing repository fields must return None"
        );

        // Repository present but number absent.
        let payload2 = serde_json::json!({
            "action": "opened",
            "repository": {"owner": {"login": "o"}, "name": "r"}
            // "number" field intentionally absent
        });
        assert!(
            handle(&make_agent(), &serde_json::to_vec(&payload2).unwrap())
                .await
                .is_none(),
            "missing number field must return None"
        );
    }

    #[tokio::test]
    async fn handle_reopened_action_is_not_skipped() {
        // `reopened` is in REVIEW_ACTIONS — handle must return Some, not None.
        let payload = serde_json::json!({
            "action": "reopened",
            "number": 8,
            "pull_request": { "draft": false, "head": { "sha": "aabbcc" } },
            "repository": {"owner": {"login": "o"}, "name": "r"}
        });
        assert!(
            handle(
                &make_agent_with_responses(vec![end_turn()]),
                &serde_json::to_vec(&payload).unwrap()
            )
            .await
            .is_some(),
            "reopened must not be skipped"
        );
    }

    #[tokio::test]
    async fn handle_new_sha_writes_dedup_marker() {
        use crate::promise_store::mock::MockPromiseStore;
        use std::sync::Arc;

        let store = Arc::new(MockPromiseStore::new());
        let mut agent = make_agent_with_responses(vec![end_turn()]);
        agent.promise_store = Some(Arc::clone(&store) as Arc<dyn crate::promise_store::PromiseRepository>);

        let payload = serde_json::json!({
            "action": "opened",
            "number": 9,
            "pull_request": { "draft": false, "head": { "sha": "newsha123" } },
            "repository": {"owner": {"login": "o"}, "name": "r"}
        });
        let _ = handle(&agent, &serde_json::to_vec(&payload).unwrap()).await;

        let snapshot = store.snapshot_promises();
        let dedup_key = "test.pr-review-sha.o.r.newsha123";
        assert!(
            snapshot.contains_key(dedup_key),
            "dedup marker must be written for new SHA; keys: {snapshot:?}"
        );
    }

    #[tokio::test]
    async fn handle_empty_sha_does_not_skip_via_dedup() {
        use crate::promise_store::mock::MockPromiseStore;
        use crate::promise_store::{AgentPromise, PromiseStatus};
        use std::sync::Arc;

        // Pre-populate a marker for the empty-sha key to prove it is never consulted.
        let store = MockPromiseStore::new();
        store.insert_promise(AgentPromise {
            id: "pr-review-sha.o.r.".to_string(), // empty sha suffix
            tenant_id: "test".to_string(),
            automation_id: String::new(),
            status: PromiseStatus::Resolved,
            messages: vec![],
            iteration: 0,
            worker_id: String::new(),
            claimed_at: 0,
            trigger: serde_json::Value::Null,
            nats_subject: String::new(),
            system_prompt: None,
            recovery_count: 0,
            checkpoint_degraded: false,
            failure_reason: None,
        });
        let mut agent = make_agent_with_responses(vec![end_turn()]);
        agent.promise_store = Some(Arc::new(store));

        // Payload deliberately omits pull_request.head.sha → head_sha = ""
        let payload = serde_json::json!({
            "action": "opened",
            "number": 10,
            "pull_request": { "draft": false, "head": {} },
            "repository": {"owner": {"login": "o"}, "name": "r"}
        });
        // With empty sha the dedup guard is bypassed → handler reaches the agent.
        assert!(
            handle(&agent, &serde_json::to_vec(&payload).unwrap())
                .await
                .is_some(),
            "empty sha must not trigger dedup skip"
        );
    }

    #[tokio::test]
    async fn handle_get_promise_error_does_not_block_review() {
        // If get_promise returns Err the handler must treat it as a cache miss
        // and proceed, not abort. The `.ok().flatten()` in the dedup guard
        // converts Err → None.
        use crate::promise_store::{AgentPromise, PromiseEntry, PromiseStoreError};
        use std::future::Future;
        use std::pin::Pin;
        use std::sync::Arc;

        struct FailingGetStore;
        impl crate::promise_store::PromiseRepository for FailingGetStore {
            fn get_promise<'a>(
                &'a self,
                _: &'a str,
                _: &'a str,
            ) -> Pin<Box<dyn Future<Output = Result<Option<PromiseEntry>, PromiseStoreError>> + Send + 'a>>
            {
                Box::pin(async { Err(PromiseStoreError("simulated get failure".into())) })
            }
            fn put_promise<'a>(
                &'a self,
                _: &'a AgentPromise,
            ) -> Pin<Box<dyn Future<Output = Result<u64, PromiseStoreError>> + Send + 'a>> {
                Box::pin(async { Ok(1) })
            }
            fn update_promise<'a>(
                &'a self, _: &'a str, _: &'a str, _: &'a AgentPromise, _: u64,
            ) -> Pin<Box<dyn Future<Output = Result<u64, PromiseStoreError>> + Send + 'a>> {
                Box::pin(async { Ok(1) })
            }
            fn get_tool_result<'a>(
                &'a self, _: &'a str, _: &'a str, _: &'a str,
            ) -> Pin<Box<dyn Future<Output = Result<Option<String>, PromiseStoreError>> + Send + 'a>> {
                Box::pin(async { Ok(None) })
            }
            fn put_tool_result<'a>(
                &'a self, _: &'a str, _: &'a str, _: &'a str, _: &'a str,
            ) -> Pin<Box<dyn Future<Output = Result<(), PromiseStoreError>> + Send + 'a>> {
                Box::pin(async { Ok(()) })
            }
            fn list_running<'a>(
                &'a self, _: &'a str,
            ) -> Pin<Box<dyn Future<Output = Result<Vec<AgentPromise>, PromiseStoreError>> + Send + 'a>> {
                Box::pin(async { Ok(vec![]) })
            }
        }

        let mut agent = make_agent_with_responses(vec![end_turn()]);
        agent.promise_store = Some(Arc::new(FailingGetStore));

        let payload = serde_json::json!({
            "action": "opened",
            "number": 11,
            "pull_request": { "draft": false, "head": { "sha": "sha-get-err" } },
            "repository": {"owner": {"login": "o"}, "name": "r"}
        });
        assert!(
            handle(&agent, &serde_json::to_vec(&payload).unwrap())
                .await
                .is_some(),
            "get_promise error must not block review"
        );
    }

    #[tokio::test]
    async fn handle_put_promise_error_does_not_block_review() {
        // If put_promise returns Err the dedup marker is lost but the handler
        // must still proceed. The `let _ =` in the dedup write ignores errors.
        use crate::promise_store::{AgentPromise, PromiseEntry, PromiseStoreError};
        use std::future::Future;
        use std::pin::Pin;
        use std::sync::Arc;

        struct FailingPutStore;
        impl crate::promise_store::PromiseRepository for FailingPutStore {
            fn get_promise<'a>(
                &'a self,
                _: &'a str,
                _: &'a str,
            ) -> Pin<Box<dyn Future<Output = Result<Option<PromiseEntry>, PromiseStoreError>> + Send + 'a>>
            {
                Box::pin(async { Ok(None) })
            }
            fn put_promise<'a>(
                &'a self,
                _: &'a AgentPromise,
            ) -> Pin<Box<dyn Future<Output = Result<u64, PromiseStoreError>> + Send + 'a>> {
                Box::pin(async { Err(PromiseStoreError("simulated put failure".into())) })
            }
            fn update_promise<'a>(
                &'a self, _: &'a str, _: &'a str, _: &'a AgentPromise, _: u64,
            ) -> Pin<Box<dyn Future<Output = Result<u64, PromiseStoreError>> + Send + 'a>> {
                Box::pin(async { Ok(1) })
            }
            fn get_tool_result<'a>(
                &'a self, _: &'a str, _: &'a str, _: &'a str,
            ) -> Pin<Box<dyn Future<Output = Result<Option<String>, PromiseStoreError>> + Send + 'a>> {
                Box::pin(async { Ok(None) })
            }
            fn put_tool_result<'a>(
                &'a self, _: &'a str, _: &'a str, _: &'a str, _: &'a str,
            ) -> Pin<Box<dyn Future<Output = Result<(), PromiseStoreError>> + Send + 'a>> {
                Box::pin(async { Ok(()) })
            }
            fn list_running<'a>(
                &'a self, _: &'a str,
            ) -> Pin<Box<dyn Future<Output = Result<Vec<AgentPromise>, PromiseStoreError>> + Send + 'a>> {
                Box::pin(async { Ok(vec![]) })
            }
        }

        let mut agent = make_agent_with_responses(vec![end_turn()]);
        agent.promise_store = Some(Arc::new(FailingPutStore));

        let payload = serde_json::json!({
            "action": "opened",
            "number": 12,
            "pull_request": { "draft": false, "head": { "sha": "sha-put-err" } },
            "repository": {"owner": {"login": "o"}, "name": "r"}
        });
        assert!(
            handle(&agent, &serde_json::to_vec(&payload).unwrap())
                .await
                .is_some(),
            "put_promise error must not block review"
        );
    }

    #[tokio::test]
    async fn handle_run_agent_success_returns_some_ok() {
        // The happy path: agent completes successfully → Some(Ok(text)).
        let payload = serde_json::json!({
            "action": "opened",
            "number": 13,
            "pull_request": { "draft": false, "head": { "sha": "happysha" } },
            "repository": {"owner": {"login": "o"}, "name": "r"}
        });
        let result = handle(
            &make_agent_with_responses(vec![end_turn()]),
            &serde_json::to_vec(&payload).unwrap(),
        )
        .await;
        assert!(
            matches!(result, Some(Ok(_))),
            "successful agent run must return Some(Ok(...)): {result:?}"
        );
    }
}

//! Linear API tools via GraphQL — all HTTP calls route through
//! `trogon-secret-proxy`.
//!
//! URL pattern: `{proxy_url}/linear/graphql`
//!
//! The proxy maps the `linear` provider prefix to `https://api.linear.app`,
//! forwarding `/linear/graphql` → `https://api.linear.app/graphql`.

use serde_json::{Value, json};

use super::{HttpClient, ToolContext};

const GRAPHQL_PATH: &str = "/linear/graphql";

/// Fetch a Linear issue by ID.
pub async fn get_issue(
    ctx: &ToolContext<impl HttpClient>,
    input: &Value,
) -> Result<String, String> {
    let issue_id = input["issue_id"].as_str().ok_or("missing issue_id")?;

    let query = json!({
        "query": "query($id: String!) {
            issue(id: $id) {
                id title description
                state { name }
                assignee { name email }
                priority
                labels { nodes { name } }
                team { name }
                createdAt updatedAt
            }
        }",
        "variables": { "id": issue_id }
    });

    let response: Value = graphql_request(ctx, &query, None).await?;
    let issue = &response["data"]["issue"];
    if issue.is_null() {
        return Err(format!("Issue {issue_id} not found"));
    }
    serde_json::to_string_pretty(issue).map_err(|e| e.to_string())
}

/// Post a comment on a Linear issue.
///
/// ## Idempotency — HTML marker + `Idempotency-Key` header
///
/// Linear supports the `Idempotency-Key` header on GraphQL mutations, so
/// duplicate requests with the same key return the original result without
/// creating a duplicate. As defence-in-depth (identical to `post_pr_comment`
/// in GitHub), when `_idempotency_key` is present this function also:
///
/// 1. Before posting, fetches existing comments on the issue and scans for an
///    HTML comment marker `<!-- trogon-idempotency-key: {key} -->` embedded in
///    any body. If found, skips the post.
/// 2. When posting, appends the marker to the body so future recovery passes
///    can detect duplicates even if the `Idempotency-Key` header window expires.
///
/// Graceful degradation: if the pre-check fetch fails, proceeds with the post.
pub async fn post_comment(
    ctx: &ToolContext<impl HttpClient>,
    input: &Value,
) -> Result<String, String> {
    let issue_id = input["issue_id"].as_str().ok_or("missing issue_id")?;
    let body = input["body"].as_str().ok_or("missing body")?;
    let idempotency_key = input["_idempotency_key"].as_str();

    // ── Idempotency pre-check ─────────────────────────────────────────────────
    if let Some(key) = idempotency_key {
        let marker = super::idempotency_marker(key);
        let check_input = json!({ "issue_id": issue_id });
        match get_comments(ctx, &check_input).await {
            Ok(json_str) => {
                if let Ok(Value::Array(comments)) = serde_json::from_str::<Value>(&json_str) {
                    let already_posted = comments.iter().any(|c| {
                        c["body"]
                            .as_str()
                            .map(|b| b.contains(&marker))
                            .unwrap_or(false)
                    });
                    if already_posted {
                        return Ok(format!(
                            "Comment already posted to {issue_id} (skipped duplicate)"
                        ));
                    }
                }
            }
            Err(_) => {
                // Retry once on transient failure before graceful degradation.
                // Short pause to avoid amplifying a transient NATS spike.
                tokio::time::sleep(std::time::Duration::from_millis(200)).await;
                match get_comments(ctx, &check_input).await {
                    Ok(json_str) => {
                        if let Ok(Value::Array(comments)) = serde_json::from_str::<Value>(&json_str) {
                            let already_posted = comments.iter().any(|c| {
                                c["body"]
                                    .as_str()
                                    .map(|b| b.contains(&marker))
                                    .unwrap_or(false)
                            });
                            if already_posted {
                                return Ok(format!(
                                    "Comment already posted to {issue_id} (skipped duplicate)"
                                ));
                            }
                        }
                    }
                    Err(_) => {
                        // Both attempts failed — proceed with the post.
                    }
                }
            }
        }
    }

    // ── Embed marker in body ─────────────────────────────────────────────────
    // Use idempotency_marker() so the embedded marker is byte-identical to the
    // one constructed in the pre-check scan above — both paths sanitise `--`
    // the same way, keeping check and write in sync.
    let effective_body = if let Some(key) = idempotency_key {
        format!("{body}\n\n{}", super::idempotency_marker(key))
    } else {
        body.to_string()
    };

    let mutation = json!({
        "query": "mutation($input: CommentCreateInput!) {
            commentCreate(input: $input) {
                success
                comment { id url }
            }
        }",
        "variables": {
            "input": { "issueId": issue_id, "body": effective_body }
        }
    });

    let response: Value = graphql_request(ctx, &mutation, idempotency_key).await?;
    let ok = response["data"]["commentCreate"]["success"]
        .as_bool()
        .unwrap_or(false);

    if ok {
        let url = response["data"]["commentCreate"]["comment"]["url"]
            .as_str()
            .unwrap_or("(no url)");
        Ok(format!("Comment posted: {url}"))
    } else {
        Err(format!("commentCreate failed: {:?}", response["errors"]))
    }
}

/// Get all comments on a Linear issue.
///
/// Returns a JSON array of comments — suitable for injecting as prior-conversation memory.
pub async fn get_comments(
    ctx: &ToolContext<impl HttpClient>,
    input: &Value,
) -> Result<String, String> {
    let issue_id = input["issue_id"].as_str().ok_or("missing issue_id")?;

    let query = json!({
        "query": "query($id: String!) {
            issue(id: $id) {
                comments {
                    nodes {
                        id body createdAt
                        user { name email }
                    }
                }
            }
        }",
        "variables": { "id": issue_id }
    });

    let response: Value = graphql_request(ctx, &query, None).await?;
    if response["data"]["issue"].is_null() {
        return Err(format!("Issue {issue_id} not found"));
    }
    let comments = &response["data"]["issue"]["comments"]["nodes"];
    serde_json::to_string_pretty(comments).map_err(|e| e.to_string())
}

/// Update a Linear issue — accepts `state_id`, `assignee_id`, and/or
/// `priority` fields; any omitted field is left unchanged.
pub async fn update_issue(
    ctx: &ToolContext<impl HttpClient>,
    input: &Value,
) -> Result<String, String> {
    let issue_id = input["issue_id"].as_str().ok_or("missing issue_id")?;
    let idempotency_key = input["_idempotency_key"].as_str();

    let mut patch = serde_json::Map::new();
    if let Some(state_id) = input["state_id"].as_str() {
        patch.insert("stateId".to_string(), json!(state_id));
    }
    if let Some(assignee_id) = input["assignee_id"].as_str() {
        patch.insert("assigneeId".to_string(), json!(assignee_id));
    }
    if let Some(priority) = input["priority"].as_u64() {
        patch.insert("priority".to_string(), json!(priority));
    }

    let mutation = json!({
        "query": "mutation($id: String!, $input: IssueUpdateInput!) {
            issueUpdate(id: $id, input: $input) {
                success
                issue { id title state { name } }
            }
        }",
        "variables": { "id": issue_id, "input": patch }
    });

    let response: Value = graphql_request(ctx, &mutation, idempotency_key).await?;
    let ok = response["data"]["issueUpdate"]["success"]
        .as_bool()
        .unwrap_or(false);

    if ok {
        let issue = &response["data"]["issueUpdate"]["issue"];
        let id = issue["id"].as_str().unwrap_or("?");
        let state = issue["state"]["name"].as_str().unwrap_or("?");
        Ok(format!("Issue {id} updated → state: {state}"))
    } else {
        Err(format!("issueUpdate failed: {:?}", response["errors"]))
    }
}

async fn graphql_request<H: HttpClient>(
    ctx: &ToolContext<H>,
    body: &Value,
    idempotency_key: Option<&str>,
) -> Result<Value, String> {
    let url = format!("{}{GRAPHQL_PATH}", ctx.proxy_url);
    let mut headers = vec![
        (
            "Authorization".to_string(),
            format!("Bearer {}", ctx.linear_token),
        ),
        ("Content-Type".to_string(), "application/json".to_string()),
    ];
    if let Some(key) = idempotency_key {
        headers.push(("Idempotency-Key".to_string(), key.to_string()));
    }
    let resp = ctx.http_client.post(&url, headers, body.clone()).await?;
    let response = serde_json::from_str::<Value>(&resp.body).map_err(|e| e.to_string())?;

    // Surface request-level errors (auth failure, rate limit, schema error)
    // before callers read data fields. Without this, a failed request where
    // "data" is null looks identical to a mutation that returned success:false,
    // making it impossible to distinguish a network/auth error from a rejected
    // mutation.
    if let Some(errors) = response["errors"].as_array()
        && !errors.is_empty()
    {
        let msg = errors
            .iter()
            .filter_map(|e| e["message"].as_str())
            .collect::<Vec<_>>()
            .join("; ");
        return Err(format!("GraphQL error: {msg}"));
    }

    Ok(response)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn make_ctx() -> crate::tools::ToolContext<crate::tools::mock::MockHttpClient> {
        crate::tools::ToolContext::for_test("http://proxy.test", "", "tok_linear", "")
    }

    // ── get_issue ─────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn get_issue_returns_pretty_json() {
        let ctx = make_ctx();
        ctx.http_client.enqueue_ok(
            200,
            json!({
                "data": {
                    "issue": {
                        "id": "ISS-1", "title": "Bug: crash on startup",
                        "description": "It crashes.", "priority": 1,
                        "state": {"name": "In Progress"},
                        "assignee": {"name": "Alice", "email": "alice@example.com"},
                        "labels": {"nodes": [{"name": "bug"}]},
                        "team": {"name": "Platform"},
                        "createdAt": "2026-01-01T00:00:00Z",
                        "updatedAt": "2026-01-02T00:00:00Z"
                    }
                }
            })
            .to_string(),
        );
        let result = get_issue(&ctx, &json!({"issue_id": "ISS-1"})).await;
        assert!(result.is_ok(), "get_issue must succeed: {result:?}");
        let v: serde_json::Value = serde_json::from_str(&result.unwrap()).unwrap();
        assert_eq!(v["title"], "Bug: crash on startup");
    }

    #[tokio::test]
    async fn get_issue_not_found_returns_err() {
        let ctx = make_ctx();
        ctx.http_client.enqueue_ok(
            200,
            json!({"data": {"issue": null}}).to_string(),
        );
        let result = get_issue(&ctx, &json!({"issue_id": "ISS-NONE"})).await;
        assert!(result.is_err(), "null issue must return Err");
        assert!(
            result.unwrap_err().contains("not found"),
            "error must mention 'not found'"
        );
    }

    // ── get_comments ──────────────────────────────────────────────────────────

    #[tokio::test]
    async fn get_comments_returns_array() {
        let ctx = make_ctx();
        ctx.http_client.enqueue_ok(
            200,
            json!({
                "data": {
                    "issue": {
                        "comments": {
                            "nodes": [
                                {"id": "c1", "body": "First comment", "createdAt": "2026-01-01T00:00:00Z", "user": {"name": "Bob", "email": "b@x.com"}},
                                {"id": "c2", "body": "Second comment", "createdAt": "2026-01-02T00:00:00Z", "user": null}
                            ]
                        }
                    }
                }
            })
            .to_string(),
        );
        let result = get_comments(&ctx, &json!({"issue_id": "ISS-2"})).await;
        assert!(result.is_ok(), "get_comments must succeed: {result:?}");
        let v: serde_json::Value = serde_json::from_str(&result.unwrap()).unwrap();
        assert_eq!(v.as_array().unwrap().len(), 2);
        assert_eq!(v[0]["body"], "First comment");
    }

    #[tokio::test]
    async fn get_comments_issue_not_found_returns_err() {
        let ctx = make_ctx();
        ctx.http_client.enqueue_ok(
            200,
            json!({"data": {"issue": null}}).to_string(),
        );
        let result = get_comments(&ctx, &json!({"issue_id": "ISS-NONE"})).await;
        assert!(result.is_err(), "null issue must return Err");
        assert!(
            result.unwrap_err().contains("not found"),
            "error must mention 'not found'"
        );
    }

    // ── update_issue ──────────────────────────────────────────────────────────

    #[tokio::test]
    async fn update_issue_with_state_id_succeeds() {
        let ctx = make_ctx();
        ctx.http_client.enqueue_ok(
            200,
            json!({
                "data": {
                    "issueUpdate": {
                        "success": true,
                        "issue": {"id": "ISS-3", "title": "Bug", "state": {"name": "Done"}}
                    }
                }
            })
            .to_string(),
        );
        let result = update_issue(
            &ctx,
            &json!({"issue_id": "ISS-3", "state_id": "state-done-uuid"}),
        )
        .await;
        assert!(result.is_ok(), "update_issue must succeed: {result:?}");
        assert!(
            result.unwrap().contains("Done"),
            "result must include new state name"
        );
    }

    #[tokio::test]
    async fn update_issue_with_all_fields_succeeds() {
        let ctx = make_ctx();
        ctx.http_client.enqueue_ok(
            200,
            json!({
                "data": {
                    "issueUpdate": {
                        "success": true,
                        "issue": {"id": "ISS-4", "title": "Task", "state": {"name": "In Review"}}
                    }
                }
            })
            .to_string(),
        );
        let result = update_issue(
            &ctx,
            &json!({
                "issue_id": "ISS-4",
                "state_id": "state-review-uuid",
                "assignee_id": "user-uuid",
                "priority": 2
            }),
        )
        .await;
        assert!(result.is_ok(), "update_issue with all fields must succeed: {result:?}");
        assert!(result.unwrap().contains("ISS-4"));
    }

    #[tokio::test]
    async fn update_issue_failure_returns_err() {
        let ctx = make_ctx();
        ctx.http_client.enqueue_ok(
            200,
            json!({
                "data": {
                    "issueUpdate": {
                        "success": false,
                        "issue": null
                    }
                },
                "errors": [{"message": "Issue not found"}]
            })
            .to_string(),
        );
        let result = update_issue(&ctx, &json!({"issue_id": "ISS-99"})).await;
        assert!(result.is_err(), "failed issueUpdate must return Err");
    }

    // ── post_comment — no idempotency key ─────────────────────────────────────

    /// First-time execution (no `_idempotency_key`) skips the pre-check and
    /// posts the comment directly without any dedup scanning.
    #[tokio::test]
    async fn post_comment_no_idempotency_key_posts_directly() {
        let ctx = make_ctx();
        // No get_comments GET enqueued — dedup must be skipped.
        ctx.http_client.enqueue_ok(
            200,
            json!({
                "data": {
                    "commentCreate": {
                        "success": true,
                        "comment": {"id": "c99", "url": "https://linear.app/acme/issue/ISS-5/comment/c99"}
                    }
                }
            })
            .to_string(),
        );
        let result = post_comment(
            &ctx,
            &json!({"issue_id": "ISS-5", "body": "Hello from agent"}),
        )
        .await;
        assert!(result.is_ok(), "first-time post must succeed: {result:?}");
        assert!(result.unwrap().contains("Comment posted"));
        assert!(
            ctx.http_client.is_empty(),
            "exactly one HTTP call (the mutation) must have been made"
        );
    }

    /// When the GraphQL mutation returns `success: false` (no GraphQL-level
    /// errors), `post_comment` must return an Err describing the failure.
    #[tokio::test]
    async fn post_comment_success_false_returns_err() {
        let ctx = make_ctx();
        // No idempotency key → no dedup GET; only the mutation POST runs.
        ctx.http_client.enqueue_ok(
            200,
            json!({
                "data": {
                    "commentCreate": {
                        "success": false,
                        "comment": null
                    }
                }
            })
            .to_string(),
        );

        let result = post_comment(
            &ctx,
            &json!({"issue_id": "ISS-42", "body": "This will fail"}),
        )
        .await;

        assert!(result.is_err(), "success:false must return Err: {result:?}");
        assert!(
            result.unwrap_err().contains("commentCreate failed"),
            "error must mention 'commentCreate failed'"
        );
    }

    /// When none of `state_id`, `assignee_id`, or `priority` are provided, the
    /// patch object is empty and the mutation is still sent (Linear accepts an
    /// empty update and returns success).
    #[tokio::test]
    async fn update_issue_empty_patch_still_sends_mutation() {
        let ctx = make_ctx();
        ctx.http_client.enqueue_ok(
            200,
            json!({
                "data": {
                    "issueUpdate": {
                        "success": true,
                        "issue": {"id": "ISS-7", "title": "Unchanged", "state": {"name": "Todo"}}
                    }
                }
            })
            .to_string(),
        );
        // Only issue_id provided — no optional patch fields.
        let result = update_issue(&ctx, &json!({"issue_id": "ISS-7"})).await;
        assert!(result.is_ok(), "empty patch must still succeed: {result:?}");
        assert!(result.unwrap().contains("ISS-7"));
    }

    /// When the first `get_comments` call fails transiently, `post_comment`
    /// sleeps 200 ms and retries. If the retry succeeds and the idempotency
    /// marker is found, the function must return early without posting.
    #[tokio::test(start_paused = true)]
    async fn post_comment_retry_finds_duplicate_on_second_get_comments_call() {
        let ctx = make_ctx();

        // First get_comments → HTTP error (triggers the 200 ms retry).
        ctx.http_client.enqueue_err("transient network error");

        // Second get_comments → returns a comment containing the marker.
        let marker = crate::tools::idempotency_marker("idem-key-abc");
        ctx.http_client.enqueue_ok(
            200,
            json!({
                "data": {
                    "issue": {
                        "comments": {
                            "nodes": [
                                {
                                    "id": "c1",
                                    "body": format!("Previously posted.\n\n{marker}"),
                                    "createdAt": "2026-01-01T00:00:00Z",
                                    "user": null
                                }
                            ]
                        }
                    }
                }
            })
            .to_string(),
        );

        let input = json!({
            "issue_id": "ISS-123",
            "body": "New comment",
            "_idempotency_key": "idem-key-abc"
        });

        let result = post_comment(&ctx, &input).await;
        assert!(result.is_ok(), "post_comment must succeed: {result:?}");
        assert!(
            result.unwrap().contains("already posted"),
            "must detect duplicate on the retried get_comments call"
        );
    }

    /// When both `get_comments` attempts fail, `post_comment` degrades
    /// gracefully and proceeds with the POST rather than returning an error.
    #[tokio::test(start_paused = true)]
    async fn post_comment_both_get_comments_fail_proceeds_with_post() {
        let ctx = make_ctx();

        // First get_comments → HTTP error.
        ctx.http_client.enqueue_err("transient error 1");
        // Second get_comments (retry) → HTTP error.
        ctx.http_client.enqueue_err("transient error 2");
        // commentCreate mutation → success.
        ctx.http_client.enqueue_ok(
            200,
            json!({
                "data": {
                    "commentCreate": {
                        "success": true,
                        "comment": {
                            "id": "c99",
                            "url": "https://linear.app/acme/issue/ISS-123/comment/c99"
                        }
                    }
                }
            })
            .to_string(),
        );

        let input = json!({
            "issue_id": "ISS-123",
            "body": "Hello world",
            "_idempotency_key": "idem-key-xyz"
        });

        let result = post_comment(&ctx, &input).await;
        assert!(result.is_ok(), "post_comment must succeed: {result:?}");
        assert!(
            result.unwrap().contains("Comment posted"),
            "must post comment after both get_comments attempts fail"
        );
    }
}

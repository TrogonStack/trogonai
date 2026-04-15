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

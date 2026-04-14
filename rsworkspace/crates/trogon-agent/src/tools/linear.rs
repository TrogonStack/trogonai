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

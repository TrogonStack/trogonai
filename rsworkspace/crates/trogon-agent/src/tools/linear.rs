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
pub async fn get_issue(ctx: &ToolContext<impl HttpClient>, input: &Value) -> Result<String, String> {
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

    let response: Value = graphql_request(ctx, &query).await?;
    serde_json::to_string_pretty(&response["data"]["issue"]).map_err(|e| e.to_string())
}

/// Post a comment on a Linear issue.
pub async fn post_comment(ctx: &ToolContext<impl HttpClient>, input: &Value) -> Result<String, String> {
    let issue_id = input["issue_id"].as_str().ok_or("missing issue_id")?;
    let body = input["body"].as_str().ok_or("missing body")?;

    let mutation = json!({
        "query": "mutation($input: CommentCreateInput!) {
            commentCreate(input: $input) {
                success
                comment { id url }
            }
        }",
        "variables": {
            "input": { "issueId": issue_id, "body": body }
        }
    });

    let response: Value = graphql_request(ctx, &mutation).await?;
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
pub async fn get_comments(ctx: &ToolContext<impl HttpClient>, input: &Value) -> Result<String, String> {
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

    let response: Value = graphql_request(ctx, &query).await?;
    let comments = &response["data"]["issue"]["comments"]["nodes"];
    serde_json::to_string_pretty(comments).map_err(|e| e.to_string())
}

/// Update a Linear issue — accepts `state_id`, `assignee_id`, and/or
/// `priority` fields; any omitted field is left unchanged.
pub async fn update_issue(ctx: &ToolContext<impl HttpClient>, input: &Value) -> Result<String, String> {
    let issue_id = input["issue_id"].as_str().ok_or("missing issue_id")?;

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

    let response: Value = graphql_request(ctx, &mutation).await?;
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

async fn graphql_request<H: HttpClient>(ctx: &ToolContext<H>, body: &Value) -> Result<Value, String> {
    let url = format!("{}{GRAPHQL_PATH}", ctx.proxy_url);
    let resp = ctx.http_client.post(&url, vec![
        ("Authorization".to_string(), format!("Bearer {}", ctx.linear_token)),
        ("Content-Type".to_string(), "application/json".to_string()),
    ], body.clone()).await.map_err(|e| e)?;
    serde_json::from_str::<Value>(&resp.body).map_err(|e| e.to_string())
}

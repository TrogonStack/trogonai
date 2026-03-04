//! Linear API tools via GraphQL — all HTTP calls route through
//! `trogon-secret-proxy`.
//!
//! URL pattern: `{proxy_url}/linear/graphql`
//!
//! The proxy maps the `linear` provider prefix to `https://api.linear.app`,
//! forwarding `/linear/graphql` → `https://api.linear.app/graphql`.

use serde_json::{Value, json};

use super::ToolContext;

const GRAPHQL_PATH: &str = "/linear/graphql";

/// Fetch a Linear issue by ID.
pub async fn get_issue(ctx: &ToolContext, input: &Value) -> Result<String, String> {
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
pub async fn post_comment(ctx: &ToolContext, input: &Value) -> Result<String, String> {
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

/// Update a Linear issue — accepts `state_id`, `assignee_id`, and/or
/// `priority` fields; any omitted field is left unchanged.
pub async fn update_issue(ctx: &ToolContext, input: &Value) -> Result<String, String> {
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

async fn graphql_request(ctx: &ToolContext, body: &Value) -> Result<Value, String> {
    let url = format!("{}{GRAPHQL_PATH}", ctx.proxy_url);

    ctx.http_client
        .post(&url)
        .header(
            "Authorization",
            format!("Bearer {}", ctx.linear_token),
        )
        .header("Content-Type", "application/json")
        .json(body)
        .send()
        .await
        .map_err(|e| e.to_string())?
        .json::<Value>()
        .await
        .map_err(|e| e.to_string())
}

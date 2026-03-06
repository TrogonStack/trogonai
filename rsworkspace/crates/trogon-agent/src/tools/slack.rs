//! Slack API tools — all HTTP calls route through `trogon-secret-proxy`.
//!
//! URL pattern: `{proxy_url}/slack/{slack_api_path}`
//!
//! The proxy maps the `slack` provider prefix to `https://slack.com/api`,
//! so `{proxy_url}/slack/chat.postMessage` becomes
//! `https://slack.com/api/chat.postMessage` with the real token resolved
//! from Vault at request time.

use serde_json::Value;

use super::{ToolContext, ToolDef, tool_def};

/// Return the Slack tool definitions to include in a handler's tool list.
pub fn slack_tool_defs() -> Vec<ToolDef> {
    vec![
        tool_def(
            "send_slack_message",
            "Send a message to a Slack channel. Use this to notify the team about important findings.",
            serde_json::json!({
                "type": "object",
                "required": ["channel", "text"],
                "properties": {
                    "channel": { "type": "string", "description": "Slack channel ID or name (e.g. #engineering)" },
                    "text":    { "type": "string", "description": "Message text (Markdown supported)" }
                }
            }),
        ),
        tool_def(
            "read_slack_channel",
            "Read recent messages from a public Slack channel.",
            serde_json::json!({
                "type": "object",
                "required": ["channel"],
                "properties": {
                    "channel": { "type": "string", "description": "Slack channel ID or name" },
                    "limit":   { "type": "integer", "description": "Number of messages to fetch (default 20)" }
                }
            }),
        ),
    ]
}

/// Send a message to a Slack channel.
pub async fn send_message(ctx: &ToolContext, input: &Value) -> Result<String, String> {
    let channel = input["channel"].as_str().ok_or("missing channel")?;
    let text = input["text"].as_str().ok_or("missing text")?;

    let url = format!("{}/slack/chat.postMessage", ctx.proxy_url);

    let response: Value = ctx
        .http_client
        .post(&url)
        .header("Authorization", format!("Bearer {}", ctx.slack_token))
        .header("Content-Type", "application/json")
        .json(&serde_json::json!({ "channel": channel, "text": text }))
        .send()
        .await
        .map_err(|e| e.to_string())?
        .json()
        .await
        .map_err(|e| e.to_string())?;

    if response["ok"].as_bool() == Some(true) {
        let ts = response["ts"].as_str().unwrap_or("unknown");
        Ok(format!("Message sent to {channel} (ts: {ts})"))
    } else {
        let error = response["error"].as_str().unwrap_or("unknown error");
        Err(format!("Slack error: {error}"))
    }
}

/// Read recent messages from a public Slack channel.
pub async fn read_channel(ctx: &ToolContext, input: &Value) -> Result<String, String> {
    let channel = input["channel"].as_str().ok_or("missing channel")?;
    let limit = input["limit"].as_u64().unwrap_or(20);

    let url = format!(
        "{}/slack/conversations.history?channel={channel}&limit={limit}",
        ctx.proxy_url,
    );

    let response: Value = ctx
        .http_client
        .get(&url)
        .header("Authorization", format!("Bearer {}", ctx.slack_token))
        .send()
        .await
        .map_err(|e| e.to_string())?
        .json()
        .await
        .map_err(|e| e.to_string())?;

    if response["ok"].as_bool() != Some(true) {
        let error = response["error"].as_str().unwrap_or("unknown error");
        return Err(format!("Slack error: {error}"));
    }

    let messages = response["messages"]
        .as_array()
        .map(|msgs| {
            msgs.iter()
                .filter_map(|m| {
                    let text = m["text"].as_str()?;
                    let user = m["user"].as_str().unwrap_or("bot");
                    let ts = m["ts"].as_str().unwrap_or("");
                    Some(format!("[{ts}] {user}: {text}"))
                })
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default();

    Ok(if messages.is_empty() {
        format!("No messages in {channel}")
    } else {
        messages
    })
}

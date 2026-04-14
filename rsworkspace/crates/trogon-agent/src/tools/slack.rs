//! Slack API tools — all HTTP calls route through `trogon-secret-proxy`.
//!
//! URL pattern: `{proxy_url}/slack/{slack_api_path}`
//!
//! The proxy maps the `slack` provider prefix to `https://slack.com/api`,
//! so `{proxy_url}/slack/chat.postMessage` becomes
//! `https://slack.com/api/chat.postMessage` with the real token resolved
//! from Vault at request time.

use serde_json::Value;
use tracing::warn;

use super::{HttpClient, ToolContext, ToolDef, tool_def};

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
///
/// ## Idempotency — Option B with graceful degradation
///
/// When `_idempotency_key` is present in the tool input (injected by the agent
/// loop during crash recovery), this function:
///
/// 1. Fetches the last 10 minutes of channel history with
///    `include_all_metadata=true`.
/// 2. **Primary check** — exact metadata match:
///    `metadata.event_type == "trogon_agent_msg"` AND
///    `metadata.event_payload.idempotency_key == key`.
///    Zero false positives: each key encodes the promise ID and tool-use ID.
/// 3. **Fallback check** — text equality.
///    Covers the case where the Slack bot token lacks the `metadata:read` scope
///    and the API omits `metadata` fields from history responses.
/// 4. If the history fetch itself fails (network error, `ok:false`), proceeds
///    with the send rather than blocking on the check.
///
/// When `_idempotency_key` is absent (normal first-time execution, not a
/// recovery path), the check is skipped entirely.
///
/// When posting, the idempotency key is embedded as Slack message metadata so
/// future recovery checks can match by key without relying on text equality:
/// `{ "metadata": { "event_type": "trogon_agent_msg",
///                  "event_payload": { "idempotency_key": "…" } } }`
pub async fn send_message(
    ctx: &ToolContext<impl HttpClient>,
    input: &Value,
) -> Result<String, String> {
    let channel = input["channel"].as_str().ok_or("missing channel")?;
    let text = input["text"].as_str().ok_or("missing text")?;
    let idempotency_key = input["_idempotency_key"].as_str();

    // ── Idempotency check (only when recovering — key present) ───────────────
    if let Some(key) = idempotency_key {
        let oldest = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
            // 49-hour window — one hour beyond TOOL_RESULTS_TTL (48 h).
            // Tool results live for 2 × PROMISE_TTL so a message posted
            // during a long run that crashes and is recovered near the end
            // of that window is still found in Slack history. Without this,
            // recovery at hour 47 would search only 25 h back and miss a
            // message posted at hour 25, causing a duplicate send.
            // limit=1000 (Slack API maximum) so the message is not missed
            // in busy channels even at the edge of the window.
            .saturating_sub(49 * 3600);

        let history_url = format!(
            "{}/slack/conversations.history?channel={channel}&oldest={oldest}&limit=1000&include_all_metadata=true",
            ctx.proxy_url,
        );

        match ctx
            .http_client
            .get(
                &history_url,
                vec![(
                    "Authorization".to_string(),
                    format!("Bearer {}", ctx.slack_token),
                )],
            )
            .await
        {
            Ok(resp) => {
                if let Ok(body) = serde_json::from_str::<Value>(&resp.body)
                    && body["ok"].as_bool() == Some(true)
                {
                    let messages = body["messages"].as_array();

                    // Primary: exact metadata key match — no false positives.
                    let metadata_match = messages
                        .map(|msgs| {
                            msgs.iter().any(|m| {
                                m["metadata"]["event_type"].as_str() == Some("trogon_agent_msg")
                                    && m["metadata"]["event_payload"]["idempotency_key"].as_str()
                                        == Some(key)
                            })
                        })
                        .unwrap_or(false);

                    if metadata_match {
                        return Ok(format!(
                            "Message already sent to {channel} (skipped duplicate)"
                        ));
                    }

                    // Warn once when messages exist but none carry a `metadata`
                    // field — the bot token is missing `metadata:read` scope.
                    // The fallback (text equality) is unreliable if the LLM
                    // regenerates different wording on recovery, so surface the
                    // misconfiguration early rather than silently duplicating.
                    if messages
                        .map(|msgs| {
                            !msgs.is_empty()
                                && msgs.iter().all(|m| m.get("metadata").is_none())
                        })
                        .unwrap_or(false)
                    {
                        warn!(
                            channel = channel,
                            "Slack history has messages but none carry a `metadata` field \
                             — bot token is likely missing the `metadata:read` OAuth scope. \
                             Idempotency dedup is falling back to text equality; duplicate \
                             messages may be sent on recovery if the LLM regenerates \
                             different wording."
                        );
                    }

                    // Fallback: text equality — handles missing metadata:read scope.
                    let text_match = messages
                        .map(|msgs| msgs.iter().any(|m| m["text"].as_str() == Some(text)))
                        .unwrap_or(false);

                    if text_match {
                        return Ok(format!(
                            "Message already sent to {channel} (skipped duplicate)"
                        ));
                    }
                }
                // History ok:false (e.g. missing scope) — proceed with send.
            }
            Err(_) => {
                // Network error on history check — proceed with send.
            }
        }
    }

    // ── Post the message ─────────────────────────────────────────────────────
    let url = format!("{}/slack/chat.postMessage", ctx.proxy_url);

    let mut body = serde_json::json!({ "channel": channel, "text": text });
    if let Some(key) = idempotency_key {
        body["metadata"] = serde_json::json!({
            "event_type": "trogon_agent_msg",
            "event_payload": { "idempotency_key": key }
        });
    }

    let resp = ctx
        .http_client
        .post(
            &url,
            vec![
                (
                    "Authorization".to_string(),
                    format!("Bearer {}", ctx.slack_token),
                ),
                ("Content-Type".to_string(), "application/json".to_string()),
            ],
            body,
        )
        .await?;
    let response: Value = serde_json::from_str(&resp.body).map_err(|e| e.to_string())?;

    if response["ok"].as_bool() == Some(true) {
        let ts = response["ts"].as_str().unwrap_or("unknown");
        Ok(format!("Message sent to {channel} (ts: {ts})"))
    } else {
        let error = response["error"].as_str().unwrap_or("unknown error");
        Err(format!("Slack error: {error}"))
    }
}

/// Read recent messages from a public Slack channel.
pub async fn read_channel(
    ctx: &ToolContext<impl HttpClient>,
    input: &Value,
) -> Result<String, String> {
    let channel = input["channel"].as_str().ok_or("missing channel")?;
    let limit = input["limit"].as_u64().unwrap_or(20);

    let url = format!(
        "{}/slack/conversations.history?channel={channel}&limit={limit}",
        ctx.proxy_url,
    );

    let resp = ctx
        .http_client
        .get(
            &url,
            vec![(
                "Authorization".to_string(),
                format!("Bearer {}", ctx.slack_token),
            )],
        )
        .await?;
    let response: Value = serde_json::from_str(&resp.body).map_err(|e| e.to_string())?;

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

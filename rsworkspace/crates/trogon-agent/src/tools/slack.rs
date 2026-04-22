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
                            !msgs.is_empty() && msgs.iter().all(|m| m.get("metadata").is_none())
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn make_ctx() -> crate::tools::ToolContext<crate::tools::mock::MockHttpClient> {
        crate::tools::ToolContext::for_test("http://proxy.test", "", "", "tok_slack")
    }

    /// When Slack history returns messages that have no `metadata` field (bot
    /// token missing `metadata:read` scope), `send_message` falls back to text
    /// equality. If a message with matching text is found, it must return early
    /// as a duplicate without posting.
    #[tokio::test]
    async fn send_message_no_metadata_read_scope_falls_back_to_text_match() {
        let ctx = make_ctx();

        // History response: messages present but none have a `metadata` field.
        // One message has text matching what we're about to send.
        ctx.http_client.enqueue_ok(
            200,
            json!({
                "ok": true,
                "messages": [
                    { "ts": "1700000001.000001", "user": "U123", "text": "Deployment complete!" },
                    { "ts": "1700000002.000002", "user": "U456", "text": "some other message" }
                ]
            })
            .to_string(),
        );
        // No POST enqueued — the function must skip posting when text matches.

        let input = json!({
            "channel": "C-ENGINEERING",
            "text": "Deployment complete!",
            "_idempotency_key": "promise-42.tool-send-1"
        });

        let result = send_message(&ctx, &input).await;
        assert!(result.is_ok(), "send_message must succeed: {result:?}");
        assert!(
            result.unwrap().contains("already sent"),
            "must detect duplicate via text fallback when metadata is absent"
        );
        assert!(
            ctx.http_client.is_empty(),
            "no POST should be made when text-fallback dedup finds a match"
        );
    }

    /// Primary metadata key match — when a message in history has the exact
    /// `event_type` and `idempotency_key` fields, the function returns early
    /// without posting.
    #[tokio::test]
    async fn send_message_metadata_primary_match_skips_post() {
        let ctx = make_ctx();

        ctx.http_client.enqueue_ok(
            200,
            json!({
                "ok": true,
                "messages": [
                    {
                        "ts": "1700000001.000001",
                        "user": "U123",
                        "text": "Deployment complete!",
                        "metadata": {
                            "event_type": "trogon_agent_msg",
                            "event_payload": { "idempotency_key": "promise-42.tool-send-1" }
                        }
                    }
                ]
            })
            .to_string(),
        );
        // No POST enqueued.

        let result = send_message(
            &ctx,
            &json!({
                "channel": "C-ENG",
                "text": "Deployment complete!",
                "_idempotency_key": "promise-42.tool-send-1"
            }),
        )
        .await;
        assert!(result.is_ok(), "send_message must succeed: {result:?}");
        assert!(
            result.unwrap().contains("already sent"),
            "must detect duplicate via metadata primary match"
        );
        assert!(
            ctx.http_client.is_empty(),
            "no POST should be made on metadata match"
        );
    }

    /// When the history GET fails with a network error, `send_message` must
    /// degrade gracefully and proceed to post rather than returning an error.
    #[tokio::test]
    async fn send_message_history_get_fails_proceeds_with_post() {
        let ctx = make_ctx();

        // History GET fails.
        ctx.http_client.enqueue_err("connection refused");
        // POST succeeds.
        ctx.http_client.enqueue_ok(
            200,
            json!({"ok": true, "ts": "1700000003.000003"}).to_string(),
        );

        let result = send_message(
            &ctx,
            &json!({
                "channel": "C-ENG",
                "text": "Hello team",
                "_idempotency_key": "key-net-err"
            }),
        )
        .await;
        assert!(
            result.is_ok(),
            "must succeed after history GET failure: {result:?}"
        );
        assert!(result.unwrap().contains("Message sent"));
    }

    /// When history returns `ok: false` (e.g. missing scope), the check is
    /// skipped and the message is posted.
    #[tokio::test]
    async fn send_message_history_ok_false_proceeds_with_post() {
        let ctx = make_ctx();

        ctx.http_client.enqueue_ok(
            200,
            json!({"ok": false, "error": "missing_scope"}).to_string(),
        );
        ctx.http_client.enqueue_ok(
            200,
            json!({"ok": true, "ts": "1700000004.000004"}).to_string(),
        );

        let result = send_message(
            &ctx,
            &json!({
                "channel": "C-ENG",
                "text": "Alert!",
                "_idempotency_key": "key-ok-false"
            }),
        )
        .await;
        assert!(
            result.is_ok(),
            "must succeed when history ok:false: {result:?}"
        );
        assert!(result.unwrap().contains("Message sent"));
    }

    /// When Slack returns `ok: false`, `read_channel` must return an Err with
    /// the error string from the response.
    #[tokio::test]
    async fn read_channel_ok_false_returns_error() {
        let ctx = make_ctx();
        ctx.http_client.enqueue_ok(
            200,
            json!({"ok": false, "error": "channel_not_found"}).to_string(),
        );
        let result = read_channel(&ctx, &json!({"channel": "C-MISSING"})).await;
        assert!(result.is_err(), "ok:false must return Err");
        assert!(
            result.unwrap_err().contains("channel_not_found"),
            "error must include Slack's error code"
        );
    }

    /// Messages that have no `text` field are silently filtered out by
    /// `filter_map`. Only messages with text contribute to the output.
    #[tokio::test]
    async fn read_channel_message_without_text_is_filtered() {
        let ctx = make_ctx();
        ctx.http_client.enqueue_ok(
            200,
            json!({
                "ok": true,
                "messages": [
                    // No `text` field — must be filtered out.
                    {"ts": "1700000001.000001", "user": "U1"},
                    // Has `text` — must appear in output.
                    {"ts": "1700000002.000002", "user": "U2", "text": "visible message"}
                ]
            })
            .to_string(),
        );
        let result = read_channel(&ctx, &json!({"channel": "C-MIXED"})).await;
        assert!(result.is_ok(), "read_channel must succeed: {result:?}");
        let body = result.unwrap();
        assert!(
            body.contains("visible message"),
            "message with text must appear"
        );
        assert!(
            !body.contains("U1"),
            "message without text must be filtered"
        );
    }

    /// When the send POST itself returns `ok: false`, `send_message` must
    /// return an Err containing Slack's error string — not a success.
    #[tokio::test]
    async fn send_message_post_ok_false_returns_err() {
        let ctx = make_ctx();

        // No idempotency key → no history GET; only the POST runs.
        ctx.http_client.enqueue_ok(
            200,
            json!({"ok": false, "error": "not_in_channel"}).to_string(),
        );

        let result =
            send_message(&ctx, &json!({"channel": "C-RESTRICTED", "text": "Hello!"})).await;

        assert!(
            result.is_err(),
            "ok:false from POST must return Err: {result:?}"
        );
        assert!(
            result.unwrap_err().contains("not_in_channel"),
            "error must include Slack's error code"
        );
    }

    /// When the history GET returns HTTP 200 but with an invalid JSON body,
    /// the `if let Ok(body) = serde_json::from_str(...)` guard fails silently
    /// and `send_message` proceeds to post the message.
    #[tokio::test]
    async fn send_message_history_invalid_json_proceeds_with_post() {
        let ctx = make_ctx();

        // History GET: 200 OK but body is malformed JSON.
        ctx.http_client.enqueue_ok(200, "not valid json {{");

        // POST: send the message normally.
        ctx.http_client.enqueue_ok(
            200,
            json!({"ok": true, "ts": "1700000005.000005"}).to_string(),
        );

        let result = send_message(
            &ctx,
            &json!({
                "channel": "C-ENG",
                "text": "Deploy complete",
                "_idempotency_key": "key-history-bad-json"
            }),
        )
        .await;

        assert!(
            result.is_ok(),
            "must proceed with send when history JSON is invalid: {result:?}"
        );
        assert!(result.unwrap().contains("Message sent"));
        assert!(
            ctx.http_client.is_empty(),
            "both GET and POST must have been consumed"
        );
    }

    /// When the HTTP response body is not valid JSON, `read_channel` must
    /// return an Err rather than panicking.
    #[tokio::test]
    async fn read_channel_json_parse_error_returns_err() {
        let ctx = make_ctx();
        ctx.http_client.enqueue_ok(200, "not valid json {{");

        let result = read_channel(&ctx, &json!({"channel": "C-ENG"})).await;
        assert!(
            result.is_err(),
            "invalid JSON response must return Err: {result:?}"
        );
    }

    /// When the channel history is empty, `read_channel` must return the
    /// sentinel string "No messages in {channel}".
    #[tokio::test]
    async fn read_channel_empty_messages_returns_no_messages_string() {
        let ctx = make_ctx();

        ctx.http_client.enqueue_ok(
            200,
            json!({
                "ok": true,
                "messages": []
            })
            .to_string(),
        );

        let input = json!({ "channel": "C-EMPTY" });

        let result = read_channel(&ctx, &input).await;
        assert!(result.is_ok(), "read_channel must succeed: {result:?}");
        assert_eq!(
            result.unwrap(),
            "No messages in C-EMPTY",
            "must return sentinel string for an empty channel"
        );
    }
}

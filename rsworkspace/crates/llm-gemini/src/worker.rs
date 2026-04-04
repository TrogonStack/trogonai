use futures::StreamExt;
use llm_types::{LlmPromptRequest, LlmStreamChunk};
use reqwest::Client as HttpClient;
use std::time::Duration;

const GEMINI_API_BASE: &str =
    "https://generativelanguage.googleapis.com/v1beta/models";
const INITIAL_BACKOFF: Duration = Duration::from_secs(1);

/// Handle one `LlmPromptRequest`: call the Gemini API, stream chunks to
/// `req.stream_inbox`, and publish a final `done=true` chunk.
pub async fn handle_request(
    nats: &async_nats::Client,
    http: &HttpClient,
    api_key: &str,
    default_model: &str,
    default_max_tokens: u32,
    retry_attempts: u32,
    req: LlmPromptRequest,
) {
    let model = req.model.as_deref().unwrap_or(default_model);
    let max_tokens = if req.max_tokens == 0 { default_max_tokens } else { req.max_tokens };
    let inbox = req.stream_inbox.clone();

    // Build Gemini `contents` array (roles: "user" | "model").
    let contents: Vec<serde_json::Value> = req
        .messages
        .iter()
        .map(|msg| {
            serde_json::json!({
                "role": gemini_role(&msg.role),
                "parts": to_gemini_parts(&msg.content),
            })
        })
        .collect();

    // Build request body.
    let mut body = serde_json::json!({
        "contents": contents,
        "generationConfig": {
            "maxOutputTokens": max_tokens,
        },
    });
    if let Some(system) = &req.system {
        body["systemInstruction"] = serde_json::json!({
            "parts": [{"text": system}],
        });
    }

    let body_bytes = match serde_json::to_vec(&body) {
        Ok(b) => b,
        Err(e) => {
            publish_error(nats, &inbox, &format!("serialize error: {e}")).await;
            return;
        }
    };

    // Gemini SSE endpoint: POST .../models/{model}:streamGenerateContent?alt=sse&key={key}
    let url = format!(
        "{GEMINI_API_BASE}/{model}:streamGenerateContent?alt=sse&key={api_key}"
    );

    // ── Retry loop ────────────────────────────────────────────────────────────

    let mut delay = INITIAL_BACKOFF;
    let mut response = None;

    for attempt in 0..retry_attempts.max(1) {
        match http
            .post(&url)
            .header("content-type", "application/json")
            .body(body_bytes.clone())
            .send()
            .await
        {
            Ok(resp) => {
                let status = resp.status();
                if status.is_success() {
                    response = Some(resp);
                    break;
                }
                let body_text = resp.text().await.unwrap_or_default();
                let retryable = status.as_u16() == 429 || status.is_server_error();
                if retryable && attempt + 1 < retry_attempts.max(1) {
                    tracing::warn!(
                        attempt = attempt + 1,
                        %status,
                        retry_in = ?delay,
                        "Gemini API retryable error, retrying"
                    );
                    tokio::time::sleep(delay).await;
                    delay *= 2;
                } else {
                    publish_error(
                        nats,
                        &inbox,
                        &format!("Gemini API {status}: {body_text}"),
                    )
                    .await;
                    return;
                }
            }
            Err(e) => {
                if attempt + 1 < retry_attempts.max(1) {
                    tracing::warn!(attempt = attempt + 1, error = %e, "HTTP error, retrying");
                    tokio::time::sleep(delay).await;
                    delay *= 2;
                } else {
                    publish_error(nats, &inbox, &format!("HTTP error: {e}")).await;
                    return;
                }
            }
        }
    }

    let resp = match response {
        Some(r) => r,
        None => {
            publish_error(nats, &inbox, "exhausted retries").await;
            return;
        }
    };

    // ── Stream SSE chunks → NATS inbox ────────────────────────────────────────
    //
    // Gemini SSE format (each event):
    //   data: {"candidates":[{"content":{"role":"model","parts":[{"text":"..."}]},...}],...}
    //
    // There is no "[DONE]" sentinel; the stream ends when the HTTP response ends.

    let mut stream = resp.bytes_stream();
    let mut line_buf = String::new();
    let mut accumulated = String::new();

    while let Some(chunk) = stream.next().await {
        let bytes = match chunk {
            Ok(b) => b,
            Err(e) => {
                publish_error(nats, &inbox, &format!("stream read error: {e}")).await;
                return;
            }
        };
        line_buf.push_str(&String::from_utf8_lossy(&bytes));

        loop {
            match line_buf.find('\n') {
                None => break,
                Some(pos) => {
                    let raw = line_buf[..pos].trim_end_matches('\r').to_string();
                    line_buf = line_buf[pos + 1..].to_string();

                    if let Some(data) = raw.strip_prefix("data: ") {
                        if let Ok(ev) = serde_json::from_str::<serde_json::Value>(data) {
                            // Extract text from all parts of the first candidate.
                            if let Some(parts) =
                                ev["candidates"][0]["content"]["parts"].as_array()
                            {
                                for part in parts {
                                    if let Some(text) =
                                        part["text"].as_str().filter(|s| !s.is_empty())
                                    {
                                        accumulated.push_str(text);
                                        let nats_chunk = LlmStreamChunk {
                                            text: text.to_string(),
                                            done: false,
                                            full_text: None,
                                            error: None,
                                        };
                                        publish_chunk(nats, &inbox, &nats_chunk).await;
                                    }
                                }
                            }

                            // Check for error in the response.
                            if let Some(err_msg) = ev["error"]["message"].as_str() {
                                publish_error(
                                    nats,
                                    &inbox,
                                    &format!("Gemini error: {err_msg}"),
                                )
                                .await;
                                return;
                            }
                        }
                    }
                }
            }
        }
    }

    // HTTP stream ended — publish done with accumulated text.
    let done_chunk = LlmStreamChunk {
        text: String::new(),
        done: true,
        full_text: Some(accumulated),
        error: None,
    };
    publish_chunk(nats, &inbox, &done_chunk).await;
}

/// Map `LlmMessage` role to Gemini role.
/// Gemini only accepts `"user"` and `"model"` (not `"assistant"`).
fn gemini_role(role: &str) -> &str {
    if role == "assistant" { "model" } else { "user" }
}

/// Convert an `LlmMessage` content value (Anthropic format) to Gemini `parts` array.
fn to_gemini_parts(content: &serde_json::Value) -> Vec<serde_json::Value> {
    match content {
        serde_json::Value::String(s) => vec![serde_json::json!({"text": s})],
        serde_json::Value::Array(blocks) => blocks
            .iter()
            .filter_map(|block| match block["type"].as_str() {
                Some("text") => Some(serde_json::json!({"text": block["text"]})),
                Some("image") => {
                    let media_type = block["source"]["media_type"].as_str()?;
                    let data = block["source"]["data"].as_str()?;
                    Some(serde_json::json!({
                        "inline_data": {"mime_type": media_type, "data": data}
                    }))
                }
                _ => None,
            })
            .collect(),
        _ => vec![],
    }
}

async fn publish_chunk(nats: &async_nats::Client, inbox: &str, chunk: &LlmStreamChunk) {
    match serde_json::to_vec(chunk) {
        Ok(bytes) => {
            if let Err(e) = nats.publish(inbox.to_string(), bytes.into()).await {
                tracing::warn!(error = %e, "Failed to publish LlmStreamChunk");
            }
        }
        Err(e) => tracing::error!(error = %e, "Failed to serialize LlmStreamChunk"),
    }
}

async fn publish_error(nats: &async_nats::Client, inbox: &str, msg: &str) {
    tracing::error!(error = %msg, "LLM worker error");
    let chunk = LlmStreamChunk {
        text: String::new(),
        done: true,
        full_text: None,
        error: Some(msg.to_string()),
    };
    publish_chunk(nats, inbox, &chunk).await;
}

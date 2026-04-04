use futures::StreamExt;
use llm_types::{LlmPromptRequest, LlmStreamChunk};
use reqwest::Client as HttpClient;
use serde::Serialize;
use std::time::Duration;

const CHAT_COMPLETIONS_URL: &str = "https://api.deepseek.com/v1/chat/completions";
const INITIAL_BACKOFF: Duration = Duration::from_secs(1);

#[derive(Serialize)]
struct ChatRequest {
    model: String,
    max_tokens: u32,
    messages: Vec<serde_json::Value>,
    stream: bool,
}

/// Handle one `LlmPromptRequest`: call the DeepSeek API (OpenAI-compatible), stream
/// chunks to `req.stream_inbox`, and publish a final `done=true` chunk.
pub async fn handle_request(
    nats: &async_nats::Client,
    http: &HttpClient,
    api_key: &str,
    default_model: &str,
    default_max_tokens: u32,
    retry_attempts: u32,
    req: LlmPromptRequest,
) {
    let model = req.model.as_deref().unwrap_or(default_model).to_string();
    let max_tokens = if req.max_tokens == 0 { default_max_tokens } else { req.max_tokens };
    let inbox = req.stream_inbox.clone();

    // Build messages: optional system message first, then conversation.
    let mut messages: Vec<serde_json::Value> = Vec::new();
    if let Some(system) = &req.system {
        messages.push(serde_json::json!({"role": "system", "content": system}));
    }
    for msg in &req.messages {
        messages.push(serde_json::json!({
            "role": msg.role,
            "content": to_openai_content(&msg.content),
        }));
    }

    let request_body = ChatRequest { model, max_tokens, messages, stream: true };

    let body_bytes = match serde_json::to_vec(&request_body) {
        Ok(b) => b,
        Err(e) => {
            publish_error(nats, &inbox, &format!("serialize error: {e}")).await;
            return;
        }
    };

    // ── Retry loop ────────────────────────────────────────────────────────────

    let mut delay = INITIAL_BACKOFF;
    let mut response = None;

    for attempt in 0..retry_attempts.max(1) {
        match http
            .post(CHAT_COMPLETIONS_URL)
            .header("authorization", format!("Bearer {api_key}"))
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
                let body = resp.text().await.unwrap_or_default();
                let retryable = status.as_u16() == 429 || status.is_server_error();
                if retryable && attempt + 1 < retry_attempts.max(1) {
                    tracing::warn!(
                        attempt = attempt + 1,
                        %status,
                        retry_in = ?delay,
                        "DeepSeek API retryable error, retrying"
                    );
                    tokio::time::sleep(delay).await;
                    delay *= 2;
                } else {
                    publish_error(nats, &inbox, &format!("DeepSeek API {status}: {body}")).await;
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
                        if data == "[DONE]" {
                            let done_chunk = LlmStreamChunk {
                                text: String::new(),
                                done: true,
                                full_text: Some(accumulated),
                                error: None,
                            };
                            publish_chunk(nats, &inbox, &done_chunk).await;
                            return;
                        }

                        if let Ok(ev) = serde_json::from_str::<serde_json::Value>(data) {
                            if let Some(text) = ev["choices"][0]["delta"]["content"]
                                .as_str()
                                .filter(|s| !s.is_empty())
                            {
                                accumulated.push_str(text);
                                let chunk = LlmStreamChunk {
                                    text: text.to_string(),
                                    done: false,
                                    full_text: None,
                                    error: None,
                                };
                                publish_chunk(nats, &inbox, &chunk).await;
                            }
                        }
                    }
                }
            }
        }
    }

    // Stream ended without [DONE] — publish what we have.
    let done_chunk = LlmStreamChunk {
        text: String::new(),
        done: true,
        full_text: Some(accumulated),
        error: None,
    };
    publish_chunk(nats, &inbox, &done_chunk).await;
}

/// Convert an `LlmMessage` content value (Anthropic format) to OpenAI-compatible content.
fn to_openai_content(content: &serde_json::Value) -> serde_json::Value {
    match content {
        serde_json::Value::String(_) => content.clone(),
        serde_json::Value::Array(blocks) => {
            let parts: Vec<serde_json::Value> = blocks
                .iter()
                .map(|block| match block["type"].as_str() {
                    Some("text") => serde_json::json!({"type": "text", "text": block["text"]}),
                    Some("image") => {
                        let media_type =
                            block["source"]["media_type"].as_str().unwrap_or("image/jpeg");
                        let data = block["source"]["data"].as_str().unwrap_or("");
                        serde_json::json!({
                            "type": "image_url",
                            "image_url": {"url": format!("data:{media_type};base64,{data}")}
                        })
                    }
                    _ => serde_json::json!({"type": "text", "text": ""}),
                })
                .collect();
            serde_json::Value::Array(parts)
        }
        _ => serde_json::Value::String(content.to_string()),
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

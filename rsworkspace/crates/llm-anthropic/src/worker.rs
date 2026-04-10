use futures::StreamExt;
use llm_types::{LlmPromptRequest, LlmStreamChunk};
use reqwest::Client as HttpClient;
use serde::Serialize;
use std::time::Duration;

const ANTHROPIC_API_URL: &str = "https://api.anthropic.com/v1/messages";
const ANTHROPIC_VERSION: &str = "2023-06-01";
const INITIAL_BACKOFF: Duration = Duration::from_secs(1);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(300);

#[derive(Serialize)]
struct MessagesRequest<'a> {
    model: &'a str,
    max_tokens: u32,
    messages: &'a [serde_json::Value],
    stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    system: Option<&'a str>,
}

/// Handle one `LlmPromptRequest`: call the Anthropic API, stream chunks to
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

    // Build wire messages: extract the content Value from each LlmMessage.
    let wire_messages: Vec<serde_json::Value> = req
        .messages
        .iter()
        .map(|m| serde_json::json!({ "role": m.role, "content": m.content }))
        .collect();

    let request_body = MessagesRequest {
        model,
        max_tokens,
        messages: &wire_messages,
        stream: true,
        system: req.system.as_deref(),
    };

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
            .post(ANTHROPIC_API_URL)
            .header("x-api-key", api_key)
            .header("anthropic-version", ANTHROPIC_VERSION)
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
                        "Anthropic API retryable error, retrying"
                    );
                    tokio::time::sleep(delay).await;
                    delay *= 2;
                } else {
                    publish_error(nats, &inbox, &format!("Anthropic API {status}: {body}")).await;
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
                            // Publish final done chunk with full text.
                            let done_chunk = LlmStreamChunk {
                                text: String::new(),
                                done: true,
                                full_text: Some(accumulated),
                                error: None,
                            };
                            publish_chunk(nats, &inbox, &done_chunk).await;
                            return;
                        }

                        // Parse content_block_delta event.
                        if let Ok(ev) = serde_json::from_str::<serde_json::Value>(data) {
                            if ev["type"] == "content_block_delta"
                                && ev["delta"]["type"] == "text_delta"
                            {
                                if let Some(text) =
                                    ev["delta"]["text"].as_str().filter(|s| !s.is_empty())
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

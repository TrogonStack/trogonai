use futures::StreamExt;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::time::Duration;
use tokio::sync::mpsc;

use crate::memory::ConversationMessage;

const ANTHROPIC_API_URL: &str = "https://api.anthropic.com/v1/messages";
const ANTHROPIC_VERSION: &str = "2023-06-01";

/// Maximum number of retry attempts for 429 / 5xx responses.
const MAX_RETRIES: u32 = 3;
/// Initial backoff delay; doubles on each attempt.
const INITIAL_BACKOFF: Duration = Duration::from_secs(1);

/// Async client for the Anthropic Messages API with SSE streaming support.
#[derive(Clone)]
pub struct ClaudeClient {
    client: Client,
    api_key: String,
    pub model: String,
    pub max_tokens: u32,
    pub system_prompt: Option<String>,
}

// ── Wire types (only what we need from the Anthropic SSE stream) ──────────────

#[derive(Serialize)]
struct MessagesRequest<'a> {
    model: &'a str,
    max_tokens: u32,
    messages: &'a [ConversationMessage],
    stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    system: Option<&'a str>,
}

#[derive(Deserialize)]
struct StreamEvent {
    #[serde(rename = "type")]
    event_type: String,
    delta: Option<DeltaContent>,
}

#[derive(Deserialize)]
struct DeltaContent {
    #[serde(rename = "type")]
    delta_type: String,
    text: Option<String>,
}

// ─────────────────────────────────────────────────────────────────────────────

impl ClaudeClient {
    pub fn new(
        api_key: String,
        model: String,
        max_tokens: u32,
        system_prompt: Option<String>,
    ) -> Self {
        Self {
            client: Client::new(),
            api_key,
            model,
            max_tokens,
            system_prompt,
        }
    }

    /// Send the request to the Anthropic API, retrying on 429 / 5xx up to
    /// `MAX_RETRIES` times with exponential back-off.
    async fn make_request(
        &self,
        messages: &[ConversationMessage],
    ) -> Result<reqwest::Response, String> {
        let request_body = MessagesRequest {
            model: &self.model,
            max_tokens: self.max_tokens,
            messages,
            stream: true,
            system: self.system_prompt.as_deref(),
        };
        let body_bytes =
            serde_json::to_vec(&request_body).map_err(|e| format!("serialize: {e}"))?;

        let mut delay = INITIAL_BACKOFF;

        for attempt in 0..MAX_RETRIES {
            let resp = self
                .client
                .post(ANTHROPIC_API_URL)
                .header("x-api-key", &self.api_key)
                .header("anthropic-version", ANTHROPIC_VERSION)
                .header("content-type", "application/json")
                .body(body_bytes.clone())
                .send()
                .await
                .map_err(|e| format!("Claude request failed: {e}"))?;

            let status = resp.status();

            if status.is_success() {
                return Ok(resp);
            }

            let body = resp.text().await.unwrap_or_default();
            let retryable = status.as_u16() == 429 || status.is_server_error();

            if retryable && attempt + 1 < MAX_RETRIES {
                tracing::warn!(
                    attempt = attempt + 1,
                    %status,
                    retry_in = ?delay,
                    "Claude API retryable error, retrying"
                );
                tokio::time::sleep(delay).await;
                delay *= 2;
                continue;
            }

            return Err(format!("Claude API error {status}: {body}"));
        }

        Err("Claude API: exhausted retries".to_string())
    }

    /// Start a streaming response from Claude.
    ///
    /// Returns an `mpsc::Receiver<String>` that will receive text chunks as
    /// they arrive from the API, followed by a `JoinHandle` that resolves to
    /// the full accumulated response text (or an error message).
    ///
    /// The receiver is closed when the stream ends or an error occurs.
    pub async fn stream_response(
        &self,
        messages: Vec<ConversationMessage>,
    ) -> Result<(mpsc::Receiver<String>, tokio::task::JoinHandle<Result<String, String>>), String>
    {
        let response = self.make_request(&messages).await?;

        let (tx, rx) = mpsc::channel::<String>(256);

        let handle = tokio::spawn(async move {
            let mut stream = response.bytes_stream();
            let mut line_buf = String::new();
            let mut accumulated = String::new();

            while let Some(chunk) = stream.next().await {
                let bytes = chunk.map_err(|e| format!("Stream read error: {e}"))?;
                line_buf.push_str(&String::from_utf8_lossy(&bytes));

                // Process every complete line in the buffer.
                loop {
                    match line_buf.find('\n') {
                        None => break,
                        Some(pos) => {
                            let raw = line_buf[..pos].trim_end_matches('\r').to_string();
                            line_buf = line_buf[pos + 1..].to_string();

                            if let Some(data) = raw.strip_prefix("data: ") {
                                if data == "[DONE]" {
                                    return Ok(accumulated);
                                }
                                if let Ok(ev) = serde_json::from_str::<StreamEvent>(data) {
                                    if ev.event_type == "content_block_delta" {
                                        if let Some(delta) = ev.delta {
                                            if delta.delta_type == "text_delta" {
                                                if let Some(text) = delta.text {
                                                    accumulated.push_str(&text);
                                                    // Best-effort send; ignore if receiver dropped.
                                                    let _ = tx.send(text).await;
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }

            Ok(accumulated)
        });

        Ok((rx, handle))
    }
}

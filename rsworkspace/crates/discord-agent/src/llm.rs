//! LLM integration module
//!
//! Handles communication with Claude API for generating responses.

use anyhow::{Context, Result};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use tracing::debug;

/// Claude API configuration
#[derive(Debug, Clone)]
pub struct ClaudeConfig {
    pub api_key: String,
    pub model: String,
    pub max_tokens: u32,
    pub temperature: f32,
}

impl Default for ClaudeConfig {
    fn default() -> Self {
        Self {
            api_key: String::new(),
            model: "claude-sonnet-4-6".to_string(),
            max_tokens: 1024,
            temperature: 1.0,
        }
    }
}

/// Claude API client
#[derive(Clone)]
pub struct ClaudeClient {
    client: Client,
    config: ClaudeConfig,
}

impl ClaudeClient {
    /// Create a new Claude client
    pub fn new(config: ClaudeConfig) -> Self {
        Self {
            client: Client::new(),
            config,
        }
    }

    /// Generate a response from Claude with streaming.
    ///
    /// Sends each text delta through `chunk_tx` as it arrives and returns the
    /// full accumulated response when the stream is complete.
    pub async fn generate_response_streaming(
        &self,
        system_prompt: &str,
        user_message: &str,
        conversation_history: &[Message],
        chunk_tx: tokio::sync::mpsc::Sender<String>,
    ) -> Result<String> {
        let mut messages = conversation_history.to_vec();
        messages.push(Message {
            role: "user".to_string(),
            content: user_message.to_string(),
        });

        let request = ClaudeRequest {
            model: self.config.model.clone(),
            max_tokens: self.config.max_tokens,
            temperature: self.config.temperature,
            system: system_prompt.to_string(),
            messages,
            stream: true,
        };

        debug!("Sending streaming request to Claude API");

        let response = self
            .client
            .post("https://api.anthropic.com/v1/messages")
            .header("x-api-key", &self.config.api_key)
            .header("anthropic-version", "2023-06-01")
            .header("content-type", "application/json")
            .json(&request)
            .send()
            .await
            .context("Failed to send streaming request to Claude API")?;

        if !response.status().is_success() {
            let status = response.status();
            let error_text = response.text().await.unwrap_or_default();
            anyhow::bail!("Claude API streaming error: {} - {}", status, error_text);
        }

        let mut accumulated = String::new();
        let mut line_buf = String::new();
        let mut response = response;

        while let Some(chunk) = response
            .chunk()
            .await
            .context("Failed to read streaming chunk")?
        {
            line_buf.push_str(&String::from_utf8_lossy(&chunk));

            // Process all complete lines in the buffer
            while let Some(pos) = line_buf.find('\n') {
                let line = line_buf[..pos].trim().to_string();
                line_buf = line_buf[pos + 1..].to_string();

                if let Some(data) = line.strip_prefix("data: ") {
                    if data == "[DONE]" {
                        break;
                    }
                    if let Ok(event) = serde_json::from_str::<SseEvent>(data) {
                        if event.event_type == "content_block_delta" {
                            if let Some(delta) = event.delta {
                                if delta.delta_type == "text_delta" {
                                    if let Some(text) = delta.text {
                                        accumulated.push_str(&text);
                                        // Best-effort send; ignore if receiver is gone
                                        let _ = chunk_tx.try_send(text);
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        debug!(
            "Streaming complete: {} chars accumulated",
            accumulated.len()
        );
        Ok(accumulated)
    }
}

/// Message in conversation history
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: String,
    pub content: String,
}

/// Claude API request
#[derive(Debug, Serialize)]
struct ClaudeRequest {
    model: String,
    max_tokens: u32,
    temperature: f32,
    system: String,
    messages: Vec<Message>,
    /// Always true: we only support streaming mode
    stream: bool,
}

// ── SSE streaming types ────────────────────────────────────────────────────

/// Top-level SSE event from the Anthropic streaming API
#[derive(Debug, Deserialize)]
struct SseEvent {
    #[serde(rename = "type")]
    event_type: String,
    delta: Option<SseDelta>,
}

/// Delta payload inside a `content_block_delta` event
#[derive(Debug, Deserialize)]
struct SseDelta {
    #[serde(rename = "type")]
    delta_type: String,
    text: Option<String>,
}

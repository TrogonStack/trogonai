//! LLM integration module
//!
//! Handles communication with Claude API for generating responses

use anyhow::{Context, Result};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use tracing::{debug, warn};

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
            model: "claude-sonnet-4-5-20250929".to_string(),
            max_tokens: 1024,
            temperature: 1.0,
        }
    }
}

/// Claude API client
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

    /// Generate a response from Claude (non-streaming)
    pub async fn generate_response(
        &self,
        system_prompt: &str,
        user_message: &str,
        conversation_history: &[Message],
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
            stream: false,
        };

        debug!("Sending request to Claude API");

        let response = self
            .client
            .post("https://api.anthropic.com/v1/messages")
            .header("x-api-key", &self.config.api_key)
            .header("anthropic-version", "2023-06-01")
            .header("content-type", "application/json")
            .json(&request)
            .send()
            .await
            .context("Failed to send request to Claude API")?;

        if !response.status().is_success() {
            let status = response.status();
            let error_text = response.text().await.unwrap_or_default();
            warn!("Claude API error: {} - {}", status, error_text);
            anyhow::bail!("Claude API error: {}", status);
        }

        let claude_response: ClaudeResponse = response
            .json()
            .await
            .context("Failed to parse Claude API response")?;

        let response_text = claude_response
            .content
            .iter()
            .filter_map(|block| {
                if block.block_type == "text" {
                    Some(block.text.clone())
                } else {
                    None
                }
            })
            .collect::<Vec<_>>()
            .join("\n");

        debug!(
            "Received response from Claude: {} tokens",
            claude_response.usage.output_tokens
        );

        Ok(response_text)
    }

    /// Generate a streaming response from Claude.
    ///
    /// Returns a channel receiver that yields text chunks as they arrive.
    /// The channel closes when the stream ends or on error.
    pub async fn generate_response_streaming(
        &self,
        system_prompt: &str,
        user_message: &str,
        conversation_history: &[Message],
    ) -> Result<tokio::sync::mpsc::Receiver<Result<String>>> {
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
            warn!("Claude API streaming error: {} - {}", status, error_text);
            anyhow::bail!("Claude API streaming error: {}", status);
        }

        let (tx, rx) = tokio::sync::mpsc::channel::<Result<String>>(64);

        tokio::spawn(async move {
            use futures::StreamExt;

            let mut byte_stream = response.bytes_stream();
            let mut buffer = String::new();

            while let Some(chunk_result) = byte_stream.next().await {
                match chunk_result {
                    Ok(bytes) => {
                        buffer.push_str(&String::from_utf8_lossy(&bytes));

                        // Process all complete lines in the buffer
                        loop {
                            if let Some(newline_pos) = buffer.find('\n') {
                                let line = buffer[..newline_pos].trim_end_matches('\r').to_string();
                                buffer = buffer[newline_pos + 1..].to_string();

                                if let Some(data) = line.strip_prefix("data: ") {
                                    if data == "[DONE]" {
                                        return;
                                    }
                                    if let Ok(event) =
                                        serde_json::from_str::<serde_json::Value>(data)
                                    {
                                        if event["type"] == "content_block_delta"
                                            && event["delta"]["type"] == "text_delta"
                                        {
                                            if let Some(text) = event["delta"]["text"].as_str() {
                                                if !text.is_empty() {
                                                    if tx.send(Ok(text.to_string())).await.is_err()
                                                    {
                                                        return; // receiver dropped
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            } else {
                                break;
                            }
                        }
                    }
                    Err(e) => {
                        let _ = tx
                            .send(Err(anyhow::anyhow!("Stream read error: {}", e)))
                            .await;
                        return;
                    }
                }
            }
        });

        Ok(rx)
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
    stream: bool,
}

/// Claude API response (non-streaming)
#[derive(Debug, Deserialize)]
struct ClaudeResponse {
    #[allow(dead_code)]
    id: String,
    #[allow(dead_code)]
    #[serde(rename = "type")]
    response_type: String,
    #[allow(dead_code)]
    role: String,
    content: Vec<ContentBlock>,
    #[allow(dead_code)]
    model: String,
    #[allow(dead_code)]
    stop_reason: Option<String>,
    usage: Usage,
}

/// Content block in response
#[derive(Debug, Deserialize)]
struct ContentBlock {
    #[serde(rename = "type")]
    block_type: String,
    text: String,
}

/// Token usage information
#[derive(Debug, Deserialize)]
struct Usage {
    #[allow(dead_code)]
    input_tokens: u32,
    output_tokens: u32,
}

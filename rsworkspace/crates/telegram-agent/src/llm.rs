//! LLM integration module
//!
//! Handles communication with Claude API for generating responses

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use reqwest::Client;
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
            model: "claude-3-5-sonnet-20241022".to_string(),
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

    /// Generate a response from Claude
    pub async fn generate_response(
        &self,
        system_prompt: &str,
        user_message: &str,
        conversation_history: &[Message],
    ) -> Result<String> {
        // Build messages array
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
        };

        debug!("Sending request to Claude API");

        let response = self.client
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

        let claude_response: ClaudeResponse = response.json().await
            .context("Failed to parse Claude API response")?;

        // Extract text from content blocks
        let response_text = claude_response.content
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

        debug!("Received response from Claude: {} tokens", claude_response.usage.output_tokens);

        Ok(response_text)
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
}

/// Claude API response
#[derive(Debug, Deserialize)]
struct ClaudeResponse {
    id: String,
    #[serde(rename = "type")]
    response_type: String,
    role: String,
    content: Vec<ContentBlock>,
    model: String,
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
    input_tokens: u32,
    output_tokens: u32,
}

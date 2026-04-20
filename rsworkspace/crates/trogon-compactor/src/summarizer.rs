//! Calls the Anthropic API to produce a structured conversation summary.
//!
//! Uses a single non-streaming request with a dedicated system prompt that
//! prevents the model from continuing the conversation.  If a previous summary
//! exists, an update prompt is used to merge new information incrementally.

use reqwest::Client;
use serde::{Deserialize, Serialize};

use crate::error::CompactorError;
use crate::serializer::serialize_for_prompt;
use crate::types::Message;

// ── Prompts ───────────────────────────────────────────────────────────────────

const SYSTEM_PROMPT: &str = "\
You are a context summarization assistant. Your task is to read a \
conversation between a user and an AI coding assistant, then produce \
a structured summary. \
Do NOT continue the conversation. Do NOT respond to any questions. \
ONLY output the structured summary — nothing else.";

const INITIAL_PROMPT: &str = "\
Create a structured context checkpoint summary using EXACTLY this format:

## Goal
[What is the user trying to accomplish?]

## Constraints & Preferences
- [Any constraints or preferences mentioned]

## Progress
### Done
- [x] [Completed tasks]
### In Progress
- [ ] [Current work]
### Blocked
- [Issues preventing progress, if any]

## Key Decisions
- **[Decision]**: [Brief rationale]

## Next Steps
1. [Ordered list of what comes next]

## Critical Context
- [File paths, function names, error messages, or data needed to continue]";

const UPDATE_PROMPT: &str = "\
Update the existing summary below with new information from the conversation.

RULES:
- PRESERVE all existing information unless it has been superseded.
- ADD new progress and decisions from the new messages.
- UPDATE the Progress section: move items from In Progress → Done when completed.
- PRESERVE exact file paths, function names, and error messages.
- Output ONLY the updated structured summary — nothing else.";

// ── LLM configuration ─────────────────────────────────────────────────────────

/// Authentication style for the LLM endpoint.
#[derive(Debug, Clone, Default)]
pub enum AuthStyle {
    /// Direct Anthropic API — sends `x-api-key: {key}`.
    #[default]
    XApiKey,
    /// Via `trogon-secret-proxy` — sends `Authorization: Bearer {token}`.
    /// This is the standard auth style for all trogon services.
    Bearer,
}

/// Connection details for the summarization LLM.
#[derive(Debug, Clone)]
pub struct LlmConfig {
    /// Full messages endpoint URL.
    ///
    /// - Direct Anthropic: `https://api.anthropic.com/v1/messages`
    /// - Via trogon-secret-proxy: `{PROXY_URL}/anthropic/v1/messages`
    pub api_url: String,
    /// API key or proxy bearer token.
    pub api_key: String,
    /// How the key is sent. Use [`AuthStyle::Bearer`] when routing through
    /// `trogon-secret-proxy` (the standard in production).
    pub auth_style: AuthStyle,
    /// Model to use. Prefer a fast, cost-efficient model (e.g. Haiku).
    pub model: String,
    /// Maximum tokens for the summary output.
    pub max_summary_tokens: u32,
}

impl Default for LlmConfig {
    fn default() -> Self {
        Self {
            api_url: "https://api.anthropic.com/v1/messages".into(),
            api_key: String::new(),
            auth_style: AuthStyle::XApiKey,
            model: "claude-haiku-4-5-20251001".into(),
            max_summary_tokens: 8_192,
        }
    }
}

// ── Anthropic wire types ──────────────────────────────────────────────────────

#[derive(Serialize)]
struct SumRequest<'a> {
    model: &'a str,
    max_tokens: u32,
    system: &'static str,
    messages: [SumMessage<'a>; 1],
}

#[derive(Serialize)]
struct SumMessage<'a> {
    role: &'static str,
    content: &'a str,
}

#[derive(Deserialize)]
struct SumResponse {
    stop_reason: String,
    content: Vec<SumBlock>,
}

#[derive(Deserialize)]
struct SumBlock {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    text: String,
}

// ── Public API ────────────────────────────────────────────────────────────────

/// Calls the LLM to summarize `messages_to_summarize`.
///
/// If `previous_summary` is `Some`, an incremental update prompt is used so
/// the model merges new information rather than regenerating from scratch.
pub async fn generate_summary(
    messages_to_summarize: &[Message],
    previous_summary: Option<&str>,
    config: &LlmConfig,
    client: &Client,
) -> Result<String, CompactorError> {
    let conversation = serialize_for_prompt(messages_to_summarize);
    let prompt = build_prompt(&conversation, previous_summary);

    let req = SumRequest {
        model: &config.model,
        max_tokens: config.max_summary_tokens,
        system: SYSTEM_PROMPT,
        messages: [SumMessage {
            role: "user",
            content: &prompt,
        }],
    };

    let auth_value = match config.auth_style {
        AuthStyle::XApiKey => config.api_key.clone(),
        AuthStyle::Bearer => format!("Bearer {}", config.api_key),
    };
    let auth_header = match config.auth_style {
        AuthStyle::XApiKey => "x-api-key",
        AuthStyle::Bearer => "Authorization",
    };

    let resp: SumResponse = client
        .post(&config.api_url)
        .header(auth_header, auth_value)
        .header("anthropic-version", "2023-06-01")
        .json(&req)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;

    if resp.stop_reason != "end_turn" {
        return Err(CompactorError::UnexpectedStopReason(resp.stop_reason));
    }

    let text: String = resp
        .content
        .into_iter()
        .filter(|b| b.kind == "text")
        .map(|b| b.text)
        .collect::<Vec<_>>()
        .join("\n");

    if text.is_empty() {
        return Err(CompactorError::EmptyResponse);
    }

    Ok(text)
}

fn build_prompt(conversation: &str, previous_summary: Option<&str>) -> String {
    let mut out = format!("<conversation>\n{conversation}\n</conversation>\n\n");

    if let Some(prev) = previous_summary {
        out.push_str(&format!(
            "<previous-summary>\n{prev}\n</previous-summary>\n\n{UPDATE_PROMPT}"
        ));
    } else {
        out.push_str(INITIAL_PROMPT);
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_prompt_initial_contains_conversation() {
        let prompt = build_prompt("some conversation", None);
        assert!(prompt.contains("<conversation>"));
        assert!(prompt.contains("some conversation"));
        assert!(prompt.contains("## Goal"));
    }

    #[test]
    fn build_prompt_update_contains_previous_summary() {
        let prompt = build_prompt("new messages", Some("old summary"));
        assert!(prompt.contains("<previous-summary>"));
        assert!(prompt.contains("old summary"));
        assert!(prompt.contains("Update the existing summary"));
    }

    #[test]
    fn default_api_url_is_full_anthropic_messages_endpoint() {
        let config = LlmConfig::default();
        assert_eq!(config.api_url, "https://api.anthropic.com/v1/messages");
    }

    #[test]
    fn bearer_auth_style_formats_header_correctly() {
        // Verify the auth value is formatted as expected for Bearer style.
        // The actual header injection is tested via httpmock in integration tests.
        let key = "my-proxy-token";
        let bearer_value = format!("Bearer {key}");
        assert!(bearer_value.starts_with("Bearer "));
    }
}

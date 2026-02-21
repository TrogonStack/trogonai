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
/// Total time budget for a single Claude request (connection + full stream).
const REQUEST_TIMEOUT: Duration = Duration::from_secs(300);

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
    messages: Vec<ApiMessage>,
    stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    system: Option<&'a str>,
}

/// Wire representation of a conversation message sent to the Anthropic API.
///
/// `content` is a plain string for text-only messages and a JSON array of
/// content blocks for multimodal messages (images + text).  Both forms are
/// accepted by the API.
#[derive(Serialize)]
struct ApiMessage {
    role: String,
    content: serde_json::Value,
}

impl From<&crate::memory::ConversationMessage> for ApiMessage {
    fn from(msg: &crate::memory::ConversationMessage) -> Self {
        let content = if msg.images.is_empty() {
            serde_json::Value::String(msg.content.clone())
        } else {
            // Build a content-block array: images first, then the text.
            let mut blocks: Vec<serde_json::Value> = msg
                .images
                .iter()
                .map(|img| {
                    serde_json::json!({
                        "type": "image",
                        "source": {
                            "type": "base64",
                            "media_type": img.media_type,
                            "data": img.base64,
                        }
                    })
                })
                .collect();
            if !msg.content.is_empty() {
                blocks.push(serde_json::json!({"type": "text", "text": msg.content}));
            }
            serde_json::Value::Array(blocks)
        };
        ApiMessage {
            role: msg.role.clone(),
            content,
        }
    }
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
        let client = Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .build()
            .expect("Failed to build reqwest client");
        Self {
            client,
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
            messages: messages.iter().map(ApiMessage::from).collect(),
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
    ) -> Result<
        (
            mpsc::Receiver<String>,
            tokio::task::JoinHandle<Result<String, String>>,
        ),
        String,
    > {
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
                                if let Ok(ev) = serde_json::from_str::<StreamEvent>(data)
                                    && ev.event_type == "content_block_delta"
                                    && let Some(delta) = ev.delta
                                    && delta.delta_type == "text_delta"
                                    && let Some(text) = delta.text
                                {
                                    accumulated.push_str(&text);
                                    // Best-effort send; ignore if receiver dropped.
                                    let _ = tx.send(text).await;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn client_stores_model() {
        let client = ClaudeClient::new(
            "key".into(),
            "claude-sonnet-4-6".into(),
            8192,
            None,
        );
        assert_eq!(client.model, "claude-sonnet-4-6");
    }

    #[test]
    fn client_stores_max_tokens() {
        let client = ClaudeClient::new(
            "key".into(),
            "claude-sonnet-4-6".into(),
            8192,
            None,
        );
        assert_eq!(client.max_tokens, 8192);
    }

    #[test]
    fn client_stores_system_prompt() {
        let client = ClaudeClient::new(
            "k".into(),
            "m".into(),
            100,
            Some("Be helpful".into()),
        );
        assert_eq!(client.system_prompt, Some("Be helpful".to_string()));
    }

    #[test]
    fn client_no_system_prompt() {
        let client = ClaudeClient::new("k".into(), "m".into(), 100, None);
        assert!(client.system_prompt.is_none());
    }

    #[test]
    fn client_is_clone() {
        let original = ClaudeClient::new(
            "key".into(),
            "claude-opus-4-6".into(),
            4096,
            None,
        );
        let cloned = original.clone();
        assert_eq!(cloned.model, original.model);
    }

    // ── ApiMessage::from ──────────────────────────────────────────────────────

    use crate::memory::{ConversationMessage, ImageData};

    fn text_msg(role: &str, content: &str) -> ConversationMessage {
        ConversationMessage {
            role: role.to_string(),
            content: content.to_string(),
            ts: None,
            images: vec![],
        }
    }

    fn img_msg(role: &str, content: &str, images: Vec<ImageData>) -> ConversationMessage {
        ConversationMessage {
            role: role.to_string(),
            content: content.to_string(),
            ts: None,
            images,
        }
    }

    fn png_image(data: &str) -> ImageData {
        ImageData {
            media_type: "image/png".to_string(),
            base64: data.to_string(),
        }
    }

    #[test]
    fn api_message_text_only_produces_string_content() {
        let msg = text_msg("user", "hello world");
        let api = ApiMessage::from(&msg);
        assert_eq!(api.role, "user");
        assert_eq!(api.content, serde_json::Value::String("hello world".to_string()));
    }

    #[test]
    fn api_message_assistant_role_preserved() {
        let msg = text_msg("assistant", "I can help");
        let api = ApiMessage::from(&msg);
        assert_eq!(api.role, "assistant");
        assert_eq!(api.content, serde_json::Value::String("I can help".to_string()));
    }

    #[test]
    fn api_message_empty_text_produces_empty_string_content() {
        let msg = text_msg("user", "");
        let api = ApiMessage::from(&msg);
        assert_eq!(api.content, serde_json::Value::String(String::new()));
    }

    #[test]
    fn api_message_with_image_produces_array_content() {
        let msg = img_msg("user", "see this", vec![png_image("abc123")]);
        let api = ApiMessage::from(&msg);
        assert!(api.content.is_array());
        let arr = api.content.as_array().unwrap();
        // image block + text block
        assert_eq!(arr.len(), 2);
    }

    #[test]
    fn api_message_image_block_comes_before_text() {
        let msg = img_msg("user", "describe this image", vec![png_image("base64data")]);
        let api = ApiMessage::from(&msg);
        let arr = api.content.as_array().unwrap();
        assert_eq!(arr[0]["type"], "image");
        assert_eq!(arr[1]["type"], "text");
        assert_eq!(arr[1]["text"], "describe this image");
    }

    #[test]
    fn api_message_image_block_has_correct_structure() {
        let msg = img_msg("user", "hi", vec![png_image("encoded_bytes")]);
        let api = ApiMessage::from(&msg);
        let arr = api.content.as_array().unwrap();
        let img = &arr[0];
        assert_eq!(img["type"], "image");
        assert_eq!(img["source"]["type"], "base64");
        assert_eq!(img["source"]["media_type"], "image/png");
        assert_eq!(img["source"]["data"], "encoded_bytes");
    }

    #[test]
    fn api_message_image_with_empty_text_omits_text_block() {
        let msg = img_msg("user", "", vec![png_image("abc")]);
        let api = ApiMessage::from(&msg);
        let arr = api.content.as_array().unwrap();
        // Only image block — no text block for empty content.
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0]["type"], "image");
    }

    #[test]
    fn api_message_multiple_images_all_appear_before_text() {
        let images = vec![
            ImageData { media_type: "image/jpeg".to_string(), base64: "jpg1".to_string() },
            ImageData { media_type: "image/png".to_string(), base64: "png2".to_string() },
        ];
        let msg = img_msg("user", "compare these", images);
        let api = ApiMessage::from(&msg);
        let arr = api.content.as_array().unwrap();
        assert_eq!(arr.len(), 3); // 2 images + 1 text
        assert_eq!(arr[0]["type"], "image");
        assert_eq!(arr[0]["source"]["media_type"], "image/jpeg");
        assert_eq!(arr[1]["type"], "image");
        assert_eq!(arr[1]["source"]["media_type"], "image/png");
        assert_eq!(arr[2]["type"], "text");
    }

    #[test]
    fn messages_request_serializes_with_stream_true() {
        // Verify the wire format sent to Claude has stream=true.
        let req = MessagesRequest {
            model: "claude-sonnet-4-6",
            max_tokens: 4096,
            messages: vec![ApiMessage::from(&text_msg("user", "hi"))],
            stream: true,
            system: None,
        };
        let json = serde_json::to_string(&req).unwrap();
        assert!(json.contains("\"stream\":true"));
        assert!(json.contains("\"model\":\"claude-sonnet-4-6\""));
        assert!(json.contains("\"max_tokens\":4096"));
    }

    #[test]
    fn messages_request_system_prompt_included_when_set() {
        let req = MessagesRequest {
            model: "claude-haiku-4-5-20251001",
            max_tokens: 1024,
            messages: vec![],
            stream: true,
            system: Some("Be concise"),
        };
        let json = serde_json::to_string(&req).unwrap();
        assert!(json.contains("\"system\":\"Be concise\""));
    }

    #[test]
    fn messages_request_system_omitted_when_none() {
        let req = MessagesRequest {
            model: "m",
            max_tokens: 100,
            messages: vec![],
            stream: true,
            system: None,
        };
        let json = serde_json::to_string(&req).unwrap();
        assert!(!json.contains("system"));
    }
}

use bytes::Bytes;
use futures_util::stream::StreamExt as _;
use futures_util::{Stream, stream};
use serde::{Deserialize, Serialize};
use tracing::{debug, warn};

/// A single message in the conversation history.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Message {
    pub role: String,
    pub content: String,
}

/// An event emitted by the streaming chat completions endpoint.
#[derive(Debug, Clone)]
pub enum XaiEvent {
    /// A text chunk from the model.
    TextDelta { text: String },
    /// The stream ended normally (`[DONE]`).
    Done,
    /// A network or API error.
    Error { message: String },
}

/// HTTP client for xAI's OpenAI-compatible chat completions API.
///
/// Does not store an API key — callers pass it per-request so sessions can use
/// individual user keys.
pub struct XaiClient {
    http: reqwest::Client,
    base_url: String,
}

impl XaiClient {
    pub fn new() -> Self {
        let base_url = std::env::var("XAI_BASE_URL")
            .unwrap_or_else(|_| "https://api.x.ai/v1".to_string());
        Self::with_base_url(base_url)
    }

    /// Construct with an explicit base URL. Useful for tests and custom proxies.
    pub fn with_base_url(base_url: impl Into<String>) -> Self {
        Self {
            http: reqwest::Client::new(),
            base_url: base_url.into(),
        }
    }

    /// Start a streaming chat completion and return a stream of `XaiEvent`s.
    ///
    /// `api_key` is the xAI bearer token to use for this specific request.
    pub async fn chat_stream(
        &self,
        model: &str,
        messages: &[Message],
        api_key: &str,
    ) -> impl Stream<Item = XaiEvent> {
        debug!(model, messages_len = messages.len(), "xai: starting chat stream");

        let result = self.start_request(model, messages, api_key).await;
        match result {
            Ok(response) => parse_sse(response.bytes_stream()).boxed_local(),
            Err(e) => {
                let msg = e.to_string();
                warn!(error = %msg, "xai: request failed");
                stream::once(async move { XaiEvent::Error { message: msg } }).boxed_local()
            }
        }
    }

    async fn start_request(
        &self,
        model: &str,
        messages: &[Message],
        api_key: &str,
    ) -> Result<reqwest::Response, String> {
        let body = serde_json::json!({
            "model": model,
            "messages": messages,
            "stream": true
        });

        let response = self
            .http
            .post(format!("{}/chat/completions", self.base_url))
            .bearer_auth(api_key)
            .json(&body)
            .send()
            .await
            .map_err(|e| e.to_string())?;

        if !response.status().is_success() {
            let status = response.status();
            let text = response.text().await.unwrap_or_default();
            warn!(status = %status, body = %text, "xai: API error response");
            return Err(format!("xAI API error {status}: {text}"));
        }

        Ok(response)
    }
}

/// Parse a raw SSE byte stream into `XaiEvent`s, accumulating partial lines.
fn parse_sse(
    bytes: impl Stream<Item = Result<Bytes, reqwest::Error>> + 'static,
) -> impl Stream<Item = XaiEvent> {
    stream::unfold(
        (bytes.boxed_local(), String::new()),
        |(mut stream, mut buf)| async move {
            loop {
                // Consume one complete line from the buffer.
                if let Some(nl) = buf.find('\n') {
                    let line = buf[..nl].trim_end_matches('\r').to_string();
                    buf = buf[nl + 1..].to_string();
                    if let Some(event) = parse_sse_line(&line) {
                        return Some((event, (stream, buf)));
                    }
                    continue;
                }

                // Need more bytes from the network.
                match stream.next().await {
                    Some(Ok(chunk)) => {
                        buf.push_str(&String::from_utf8_lossy(&chunk));
                    }
                    Some(Err(e)) => {
                        warn!(error = %e, "xai: stream read error");
                        return Some((
                            XaiEvent::Error { message: e.to_string() },
                            (stream, String::new()),
                        ));
                    }
                    None => {
                        // Flush any remaining (unterminated) line.
                        let line = buf.trim().to_string();
                        if let Some(event) = parse_sse_line(&line) {
                            return Some((event, (stream, String::new())));
                        }
                        return None;
                    }
                }
            }
        },
    )
}

fn parse_sse_line(line: &str) -> Option<XaiEvent> {
    let data = line.strip_prefix("data: ")?;
    if data == "[DONE]" {
        return Some(XaiEvent::Done);
    }
    match serde_json::from_str::<serde_json::Value>(data) {
        Ok(val) => {
            let text = val["choices"][0]["delta"]["content"]
                .as_str()
                .unwrap_or("");
            if text.is_empty() {
                None
            } else {
                Some(XaiEvent::TextDelta { text: text.to_string() })
            }
        }
        Err(e) => {
            warn!(error = %e, data, "xai: failed to parse SSE data line");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chunk(text: &str) -> String {
        format!(
            r#"data: {{"choices":[{{"delta":{{"content":"{text}"}}}}]}}"#
        )
    }

    #[test]
    fn text_delta() {
        let line = chunk("hello");
        let event = parse_sse_line(&line).unwrap();
        assert!(matches!(event, XaiEvent::TextDelta { text } if text == "hello"));
    }

    #[test]
    fn done_signal() {
        let event = parse_sse_line("data: [DONE]").unwrap();
        assert!(matches!(event, XaiEvent::Done));
    }

    #[test]
    fn non_data_prefix_returns_none() {
        assert!(parse_sse_line("event: message").is_none());
        assert!(parse_sse_line(": keep-alive").is_none());
        assert!(parse_sse_line("").is_none());
    }

    #[test]
    fn empty_content_returns_none() {
        let line = r#"data: {"choices":[{"delta":{"content":""}}]}"#;
        assert!(parse_sse_line(line).is_none());
    }

    #[test]
    fn missing_content_field_returns_none() {
        let line = r#"data: {"choices":[{"delta":{}}]}"#;
        assert!(parse_sse_line(line).is_none());
    }

    #[test]
    fn invalid_json_returns_none() {
        assert!(parse_sse_line("data: {not valid json}").is_none());
    }

    #[test]
    fn role_only_delta_returns_none() {
        let line = r#"data: {"choices":[{"delta":{"role":"assistant"}}]}"#;
        assert!(parse_sse_line(line).is_none());
    }
}

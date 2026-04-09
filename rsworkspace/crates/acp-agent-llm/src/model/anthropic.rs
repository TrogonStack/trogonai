use super::{ChatMessage, ChatRole, CompletionEvent, LanguageModel, StopReason};
use crate::api_key::ApiKey;
use crate::error::CompletionError;
use crate::model_id::ModelId;
use crate::provider_name::ProviderName;
use futures::StreamExt;
use reqwest::Client;
use tokio::sync::{mpsc, watch};

pub struct AnthropicModelConfig {
    pub id: String,
    pub max_tokens: u64,
    pub api_key: ApiKey,
    pub base_url: String,
    pub client: Client,
    pub supports_tools: bool,
    pub supports_images: bool,
    pub supports_thinking: bool,
}

pub struct AnthropicModel {
    id: ModelId,
    provider: ProviderName,
    max_tokens: u64,
    client: Client,
    api_key: ApiKey,
    base_url: String,
    supports_tools: bool,
    supports_images: bool,
    supports_thinking: bool,
}

impl AnthropicModel {
    pub fn new(config: AnthropicModelConfig) -> Self {
        Self {
            id: ModelId::new(config.id).expect("model id from API should be non-empty"),
            provider: ProviderName::new("anthropic").expect("known provider"),
            max_tokens: config.max_tokens,
            client: config.client,
            api_key: config.api_key,
            base_url: config.base_url,
            supports_tools: config.supports_tools,
            supports_images: config.supports_images,
            supports_thinking: config.supports_thinking,
        }
    }
}

#[async_trait::async_trait(?Send)]
impl LanguageModel for AnthropicModel {
    fn id(&self) -> &ModelId {
        &self.id
    }
    fn provider_id(&self) -> &ProviderName {
        &self.provider
    }
    fn max_token_count(&self) -> u64 {
        self.max_tokens
    }
    fn supports_tools(&self) -> bool {
        self.supports_tools
    }
    fn supports_images(&self) -> bool {
        self.supports_images
    }
    fn supports_thinking(&self) -> bool {
        self.supports_thinking
    }

    async fn stream_completion(
        &self,
        messages: &[ChatMessage],
        event_tx: mpsc::Sender<CompletionEvent>,
        mut cancel: watch::Receiver<bool>,
    ) -> Result<(), CompletionError> {
        let api_messages: Vec<serde_json::Value> = messages
            .iter()
            .filter(|m| m.role != ChatRole::System)
            .map(|m| {
                serde_json::json!({
                    "role": match m.role {
                        ChatRole::User => "user",
                        ChatRole::Assistant => "assistant",
                        ChatRole::System => unreachable!(),
                    },
                    "content": m.content,
                })
            })
            .collect();

        let system = messages
            .iter()
            .find(|m| m.role == ChatRole::System)
            .map(|m| m.content.as_str());

        let mut body = serde_json::json!({
            "model": self.id.as_str(),
            "messages": api_messages,
            "max_tokens": 4096,
            "stream": true,
        });
        if let Some(sys) = system {
            body["system"] = serde_json::json!(sys);
        }

        let response = self
            .client
            .post(format!("{}/v1/messages", self.base_url))
            .header("x-api-key", self.api_key.as_str())
            .header("anthropic-version", "2023-06-01")
            .header("content-type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(CompletionError::Http)?;

        if !response.status().is_success() {
            let status = response.status().as_u16();
            let text = response.text().await.unwrap_or_default();
            return Err(CompletionError::Api {
                status,
                message: text,
            });
        }

        let mut stream = response.bytes_stream();
        let mut buffer = String::new();

        loop {
            tokio::select! {
                chunk = stream.next() => {
                    match chunk {
                        Some(Ok(bytes)) => {
                            buffer.push_str(&String::from_utf8_lossy(&bytes));
                            self.parse_sse_buffer(&mut buffer, &event_tx).await;
                        }
                        Some(Err(e)) => return Err(CompletionError::Http(e)),
                        None => break,
                    }
                }
                _ = cancel.changed() => {
                    if *cancel.borrow() {
                        return Err(CompletionError::Cancelled);
                    }
                }
            }
        }

        Ok(())
    }
}

impl AnthropicModel {
    async fn parse_sse_buffer(
        &self,
        buffer: &mut String,
        event_tx: &mpsc::Sender<CompletionEvent>,
    ) {
        while let Some(line_end) = buffer.find('\n') {
            let line = buffer[..line_end].trim_end().to_string();
            *buffer = buffer[line_end + 1..].to_string();

            let Some(data) = line.strip_prefix("data: ") else {
                continue;
            };
            if data == "[DONE]" {
                break;
            }

            let Ok(event) = serde_json::from_str::<serde_json::Value>(data) else {
                continue;
            };

            if event["type"] == "content_block_delta"
                && let Some(text) = event["delta"]["text"].as_str()
            {
                let _ = event_tx.send(CompletionEvent::Text(text.to_string())).await;
            }

            if event["type"] == "message_delta" {
                if let Some(reason) = event["delta"]["stop_reason"].as_str() {
                    let stop = match reason {
                        "end_turn" => StopReason::EndTurn,
                        "max_tokens" => StopReason::MaxTokens,
                        other => StopReason::Other(other.to_string()),
                    };
                    let _ = event_tx.send(CompletionEvent::Stop(stop)).await;
                }
                if let (Some(input), Some(output)) = (
                    event["usage"]["input_tokens"].as_u64(),
                    event["usage"]["output_tokens"].as_u64(),
                ) {
                    let _ = event_tx
                        .send(CompletionEvent::Usage {
                            input_tokens: input,
                            output_tokens: output,
                        })
                        .await;
                }
            }
        }
    }
}

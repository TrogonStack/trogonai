use super::{ChatMessage, ChatRole, CompletionEvent, LanguageModel, StopReason};
use crate::api_key::ApiKey;
use crate::error::CompletionError;
use crate::model_id::ModelId;
use crate::provider_name::ProviderName;
use futures::StreamExt;
use reqwest::Client;
use tokio::sync::{mpsc, watch};

pub struct OpenAiModel {
    id: ModelId,
    provider: ProviderName,
    max_tokens: u64,
    client: Client,
    api_key: ApiKey,
    base_url: String,
}

impl OpenAiModel {
    pub fn new(
        id: &str,
        max_tokens: u64,
        api_key: ApiKey,
        base_url: String,
        client: Client,
    ) -> Self {
        Self {
            id: ModelId::new(id).expect("model id from API should be non-empty"),
            provider: ProviderName::new("openai").expect("known provider"),
            max_tokens,
            client,
            api_key,
            base_url,
        }
    }
}

#[async_trait::async_trait(?Send)]
impl LanguageModel for OpenAiModel {
    fn id(&self) -> &ModelId {
        &self.id
    }
    fn provider_id(&self) -> &ProviderName {
        &self.provider
    }
    fn max_token_count(&self) -> u64 {
        self.max_tokens
    }

    async fn stream_completion(
        &self,
        messages: &[ChatMessage],
        event_tx: mpsc::Sender<CompletionEvent>,
        mut cancel: watch::Receiver<bool>,
    ) -> Result<(), CompletionError> {
        let api_messages: Vec<serde_json::Value> = messages
            .iter()
            .map(|m| {
                serde_json::json!({
                    "role": match m.role {
                        ChatRole::System => "system",
                        ChatRole::User => "user",
                        ChatRole::Assistant => "assistant",
                    },
                    "content": m.content,
                })
            })
            .collect();

        let body = serde_json::json!({
            "model": self.id.as_str(),
            "messages": api_messages,
            "stream": true,
        });

        let response = self
            .client
            .post(format!("{}/v1/chat/completions", self.base_url))
            .header("authorization", format!("Bearer {}", self.api_key.as_str()))
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

impl OpenAiModel {
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

            // OpenAI streams: choices[0].delta.content for text
            if let Some(content) = event["choices"][0]["delta"]["content"].as_str()
                && !content.is_empty()
            {
                let _ = event_tx
                    .send(CompletionEvent::Text(content.to_string()))
                    .await;
            }

            // choices[0].finish_reason for stop
            if let Some(reason) = event["choices"][0]["finish_reason"].as_str() {
                let stop = match reason {
                    "stop" => StopReason::EndTurn,
                    "length" => StopReason::MaxTokens,
                    other => StopReason::Other(other.to_string()),
                };
                let _ = event_tx.send(CompletionEvent::Stop(stop)).await;
            }

            // usage object (sent in final chunk if stream_options.include_usage is true)
            if let (Some(input), Some(output)) = (
                event["usage"]["prompt_tokens"].as_u64(),
                event["usage"]["completion_tokens"].as_u64(),
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

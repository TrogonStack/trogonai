use super::LanguageModelProvider;
use crate::api_key::ApiKey;
use crate::error::CompletionError;
use crate::model::LanguageModel;
use crate::model::anthropic::{AnthropicModel, AnthropicModelConfig};
use crate::model_id::ModelId;
use crate::provider_name::ProviderName;
use reqwest::Client;

/// Manages Anthropic authentication and available models.
/// Models are fetched from `GET /v1/models` at startup — not hardcoded.
pub struct AnthropicProvider {
    name: ProviderName,
    api_key: ApiKey,
    base_url: String,
    client: Client,
    models: Vec<AnthropicModel>,
}

impl AnthropicProvider {
    pub fn new(api_key: ApiKey, base_url: Option<String>) -> Self {
        let base = base_url.unwrap_or_else(|| "https://api.anthropic.com".to_string());
        Self {
            name: ProviderName::new("anthropic").expect("known provider"),
            api_key,
            base_url: base,
            client: Client::new(),
            models: Vec::new(),
        }
    }
}

#[async_trait::async_trait(?Send)]
impl LanguageModelProvider for AnthropicProvider {
    fn id(&self) -> &ProviderName {
        &self.name
    }

    fn is_authenticated(&self) -> bool {
        true
    }

    fn default_model(&self) -> ModelId {
        ModelId::new("claude-sonnet-4-6").expect("known model id")
    }

    fn provided_models(&self) -> Vec<ModelId> {
        self.models.iter().map(|m| m.id().clone()).collect()
    }

    fn model(&self, id: &ModelId) -> Option<&dyn LanguageModel> {
        self.models
            .iter()
            .find(|m| m.id() == id)
            .map(|m| m as &dyn LanguageModel)
    }

    async fn fetch_models(&mut self) -> Result<(), CompletionError> {
        let response = self
            .client
            .get(format!("{}/v1/models", self.base_url))
            .header("x-api-key", self.api_key.as_str())
            .header("anthropic-version", "2023-06-01")
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

        let body: serde_json::Value = response.json().await.map_err(CompletionError::Http)?;

        self.models = body["data"]
            .as_array()
            .unwrap_or(&vec![])
            .iter()
            .filter_map(|m| {
                let id = m["id"].as_str()?;
                let max_input = m["max_input_tokens"].as_u64().unwrap_or(200_000);
                let capabilities = &m["capabilities"];
                Some(AnthropicModel::new(AnthropicModelConfig {
                    id: id.to_string(),
                    max_tokens: max_input,
                    api_key: self.api_key.clone(),
                    base_url: self.base_url.clone(),
                    client: self.client.clone(),
                    supports_tools: capabilities["tool_use"].as_bool().unwrap_or(false),
                    supports_images: capabilities["image_input"].as_bool().unwrap_or(false),
                    supports_thinking: capabilities["thinking"].as_bool().unwrap_or(false),
                }))
            })
            .collect();

        Ok(())
    }
}

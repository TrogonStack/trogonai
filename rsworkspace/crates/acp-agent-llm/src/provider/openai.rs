use super::LanguageModelProvider;
use crate::api_key::ApiKey;
use crate::error::CompletionError;
use crate::model::LanguageModel;
use crate::model::openai::OpenAiModel;
use crate::model_id::ModelId;
use crate::provider_name::ProviderName;
use reqwest::Client;

/// Manages OpenAI authentication and available models.
/// Models are fetched from `GET /v1/models` at startup.
pub struct OpenAiProvider {
    name: ProviderName,
    api_key: ApiKey,
    base_url: String,
    client: Client,
    models: Vec<OpenAiModel>,
}

impl OpenAiProvider {
    pub fn new(api_key: ApiKey, base_url: Option<String>) -> Self {
        let base = base_url.unwrap_or_else(|| "https://api.openai.com".to_string());
        Self {
            name: ProviderName::new("openai").expect("known provider"),
            api_key,
            base_url: base,
            client: Client::new(),
            models: Vec::new(),
        }
    }
}

#[async_trait::async_trait(?Send)]
impl LanguageModelProvider for OpenAiProvider {
    fn id(&self) -> &ProviderName {
        &self.name
    }

    fn is_authenticated(&self) -> bool {
        true
    }

    fn default_model(&self) -> ModelId {
        ModelId::new("gpt-4o").expect("known model id")
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
            .header("authorization", format!("Bearer {}", self.api_key.as_str()))
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

        // OpenAI's /v1/models returns a list of model objects.
        // Token limits are not included in the response — use sensible defaults.
        self.models = body["data"]
            .as_array()
            .unwrap_or(&vec![])
            .iter()
            .filter_map(|m| {
                let id = m["id"].as_str()?;
                // Default to 128k context — OpenAI doesn't expose this in the list endpoint
                let max_tokens = 128_000u64;
                Some(OpenAiModel::new(
                    id,
                    max_tokens,
                    self.api_key.clone(),
                    self.base_url.clone(),
                    self.client.clone(),
                ))
            })
            .collect();

        Ok(())
    }
}

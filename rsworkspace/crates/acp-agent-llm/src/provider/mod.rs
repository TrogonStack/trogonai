pub mod anthropic;
pub mod openai;

use crate::error::CompletionError;
use crate::model::LanguageModel;
use crate::model_id::ModelId;
use crate::provider_name::ProviderName;

/// Manages authentication and lists available models for a provider.
/// One instance per provider (Anthropic, OpenAI, etc.).
///
/// Models are fetched from the provider's API at startup via `fetch_models()`,
/// not hardcoded.
#[async_trait::async_trait(?Send)]
pub trait LanguageModelProvider {
    /// Provider identity (e.g., "anthropic").
    fn id(&self) -> &ProviderName;

    /// Whether the provider has a valid API key configured.
    fn is_authenticated(&self) -> bool;

    /// The provider's recommended default model.
    fn default_model(&self) -> ModelId;

    /// All models this provider offers.
    fn provided_models(&self) -> Vec<ModelId>;

    /// Look up a specific model by ID.
    fn model(&self, id: &ModelId) -> Option<&dyn LanguageModel>;

    /// Fetch available models from the provider's API and populate the internal registry.
    /// Called once at startup. Both Anthropic (`GET /v1/models`) and OpenAI (`GET /v1/models`)
    /// support this.
    async fn fetch_models(&mut self) -> Result<(), CompletionError>;
}

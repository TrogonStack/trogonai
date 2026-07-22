use confique::Config;
use trogon_service_config::NatsConfigSection;

/// Full configuration for the `trogon-router` binary, loaded from environment
/// variables and an optional TOML config file.
#[derive(Config, Clone, Debug)]
pub struct RouterConfig {
    #[config(nested)]
    pub nats: NatsConfigSection,

    /// Base URL of the OpenAI-compatible LLM endpoint.
    ///
    /// Defaults to the xAI Grok API. Set `ROUTER_LLM_API_URL` to override.
    #[config(env = "ROUTER_LLM_API_URL", default = "https://api.x.ai/v1")]
    pub llm_api_url: String,

    /// Bearer token for the LLM API (`Authorization: Bearer <token>`).
    #[config(env = "ROUTER_LLM_API_KEY")]
    pub llm_api_key: String,

    /// Model identifier passed in the `model` field of the chat-completions request.
    #[config(env = "ROUTER_LLM_MODEL", default = "grok-3-mini")]
    pub llm_model: String,

    /// NATS subject pattern the router subscribes to for incoming events.
    #[config(env = "ROUTER_EVENTS_SUBJECT", default = "trogon.events.>")]
    pub events_subject: String,
}

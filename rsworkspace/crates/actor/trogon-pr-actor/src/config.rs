use confique::Config;
use trogon_service_config::NatsConfigSection;

/// Configuration for the `trogon-pr-actor` binary.
///
/// All fields are loaded from environment variables and an optional TOML
/// config file (path passed via `--config`).
#[derive(Config, Clone, Debug)]
pub struct PrActorConfig {
    #[config(nested)]
    pub nats: NatsConfigSection,

    /// Base URL of the OpenAI-compatible LLM endpoint used for code review.
    #[config(env = "PR_ACTOR_LLM_API_URL", default = "https://api.x.ai/v1")]
    pub llm_api_url: String,

    /// Bearer token for the LLM API (`Authorization: Bearer <token>`).
    #[config(env = "PR_ACTOR_LLM_API_KEY", default = "")]
    pub llm_api_key: String,

    /// Model identifier passed to the LLM (e.g. `"grok-3-mini"`).
    #[config(env = "PR_ACTOR_LLM_MODEL", default = "grok-3-mini")]
    pub llm_model: String,

    /// GitHub personal access token with `repo` scope for reading diffs and
    /// posting review comments.
    #[config(env = "PR_ACTOR_GITHUB_TOKEN", default = "")]
    pub github_token: String,
}

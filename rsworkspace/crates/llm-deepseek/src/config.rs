use trogon_nats::NatsConfig;
use trogon_std::env::ReadEnv;

pub struct LlmDeepSeekConfig {
    /// DeepSeek API key (`DEEPSEEK_API_KEY`).
    pub api_key: String,
    /// Default model when the request does not specify one.
    /// Env: `DEEPSEEK_DEFAULT_MODEL`. Default: `"deepseek-chat"`.
    pub default_model: String,
    /// Default max tokens. Env: `DEEPSEEK_DEFAULT_MAX_TOKENS`. Default: `8192`.
    pub default_max_tokens: u32,
    /// NATS connection settings.
    pub nats: NatsConfig,
    /// Optional account-ID namespace for multi-workspace deployments.
    /// Env: `ACCOUNT_ID`.
    pub account_id: Option<String>,
    /// Number of retry attempts on 429 / 5xx errors.
    /// Env: `DEEPSEEK_RETRY_ATTEMPTS`. Default: `3`.
    pub retry_attempts: u32,
}

impl LlmDeepSeekConfig {
    pub fn from_env<E: ReadEnv>(env: &E) -> Self {
        let api_key = env
            .var("DEEPSEEK_API_KEY")
            .expect("DEEPSEEK_API_KEY is required");

        let default_model = env
            .var("DEEPSEEK_DEFAULT_MODEL")
            .unwrap_or_else(|_| "deepseek-chat".to_string());

        let default_max_tokens = env
            .var("DEEPSEEK_DEFAULT_MAX_TOKENS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(8192);

        let account_id = env.var("ACCOUNT_ID").ok().filter(|v| !v.is_empty());

        let retry_attempts = env
            .var("DEEPSEEK_RETRY_ATTEMPTS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(3);

        Self {
            api_key,
            default_model,
            default_max_tokens,
            account_id,
            retry_attempts,
            nats: NatsConfig::from_env(env),
        }
    }
}

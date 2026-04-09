use crate::api_key::ApiKey;
use crate::model_id::ModelId;
use crate::provider_name::ProviderName;
use acp_nats::{AcpPrefix, Config as AcpConfig, NatsConfig};
use clap::Parser;
use trogon_std::ParseArgs;
use trogon_std::env::ReadEnv;

#[derive(Parser, Debug, Clone)]
#[command(name = "acp-agent-llm")]
#[command(about = "Model-agnostic LLM agent for ACP over NATS", long_about = None)]
pub struct Args {
    #[arg(long = "acp-prefix")]
    pub acp_prefix: Option<String>,

    #[arg(long = "provider")]
    pub provider: Option<String>,

    #[arg(long = "model")]
    pub model: Option<String>,

    #[arg(long = "base-url")]
    pub base_url: Option<String>,
}

pub struct LlmConfig {
    pub acp: AcpConfig,
    pub default_provider: ProviderName,
    pub default_model: ModelId,
    pub anthropic_api_key: Option<ApiKey>,
    pub openai_api_key: Option<ApiKey>,
    pub base_url: Option<String>,
}

#[derive(Debug)]
pub enum ConfigError {
    AcpPrefix(acp_nats::AcpPrefixError),
    Provider(crate::provider_name::UnknownProvider),
    Model(crate::model_id::EmptyModelId),
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::AcpPrefix(e) => write!(f, "invalid ACP prefix: {e}"),
            Self::Provider(e) => write!(f, "invalid provider: {e}"),
            Self::Model(e) => write!(f, "invalid model: {e}"),
        }
    }
}

impl std::error::Error for ConfigError {}

impl From<acp_nats::AcpPrefixError> for ConfigError {
    fn from(e: acp_nats::AcpPrefixError) -> Self {
        Self::AcpPrefix(e)
    }
}

impl From<crate::provider_name::UnknownProvider> for ConfigError {
    fn from(e: crate::provider_name::UnknownProvider) -> Self {
        Self::Provider(e)
    }
}

impl From<crate::model_id::EmptyModelId> for ConfigError {
    fn from(e: crate::model_id::EmptyModelId) -> Self {
        Self::Model(e)
    }
}

pub fn build_config<P: ParseArgs<Args = Args>, E: ReadEnv>(
    parser: &P,
    env: &E,
) -> Result<LlmConfig, ConfigError> {
    let args = parser.parse_args();
    build_config_from_args(args, env)
}

fn build_config_from_args<E: ReadEnv>(args: Args, env: &E) -> Result<LlmConfig, ConfigError> {
    let raw_prefix = args
        .acp_prefix
        .or_else(|| env.var(acp_nats::ENV_ACP_PREFIX).ok())
        .unwrap_or_else(|| acp_nats::DEFAULT_ACP_PREFIX.to_string());
    let prefix = AcpPrefix::new(raw_prefix)?;

    let provider_str = args
        .provider
        .or_else(|| env.var("LLM_PROVIDER").ok())
        .unwrap_or_else(|| "anthropic".to_string());
    let provider = ProviderName::new(provider_str)?;

    let default_model = match args.model.or_else(|| env.var("LLM_MODEL").ok()) {
        Some(m) => ModelId::new(m)?,
        None => match provider.as_str() {
            "anthropic" => ModelId::new("claude-sonnet-4-6").expect("known model"),
            "openai" => ModelId::new("gpt-4o").expect("known model"),
            _ => unreachable!(),
        },
    };

    let anthropic_key = env
        .var("ANTHROPIC_API_KEY")
        .ok()
        .and_then(|k| ApiKey::new(k).ok());
    let openai_key = env
        .var("OPENAI_API_KEY")
        .ok()
        .and_then(|k| ApiKey::new(k).ok());
    let base_url = args.base_url.or_else(|| env.var("LLM_BASE_URL").ok());

    let acp = AcpConfig::with_prefix(prefix, NatsConfig::from_env(env));

    Ok(LlmConfig {
        acp,
        default_provider: provider,
        default_model,
        anthropic_api_key: anthropic_key,
        openai_api_key: openai_key,
        base_url,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use trogon_std::FixedArgs;
    use trogon_std::env::InMemoryEnv;

    #[test]
    fn default_config() {
        let env = InMemoryEnv::new();
        let parser = FixedArgs(Args {
            acp_prefix: None,
            provider: None,
            model: None,
            base_url: None,
        });
        let config = build_config(&parser, &env).unwrap();
        assert_eq!(config.default_provider.as_str(), "anthropic");
        assert_eq!(config.default_model.as_str(), "claude-sonnet-4-6");
        assert!(config.anthropic_api_key.is_none());
    }

    #[test]
    fn provider_from_env() {
        let env = InMemoryEnv::new();
        env.set("LLM_PROVIDER", "openai");
        let parser = FixedArgs(Args {
            acp_prefix: None,
            provider: None,
            model: None,
            base_url: None,
        });
        let config = build_config(&parser, &env).unwrap();
        assert_eq!(config.default_provider.as_str(), "openai");
        assert_eq!(config.default_model.as_str(), "gpt-4o");
    }

    #[test]
    fn api_key_from_env() {
        let env = InMemoryEnv::new();
        env.set("ANTHROPIC_API_KEY", "sk-test");
        let parser = FixedArgs(Args {
            acp_prefix: None,
            provider: None,
            model: None,
            base_url: None,
        });
        let config = build_config(&parser, &env).unwrap();
        assert!(config.anthropic_api_key.is_some());
    }

    #[test]
    fn args_override_env() {
        let env = InMemoryEnv::new();
        env.set("LLM_PROVIDER", "openai");
        let parser = FixedArgs(Args {
            acp_prefix: None,
            provider: Some("anthropic".to_string()),
            model: Some("claude-opus-4-6".to_string()),
            base_url: None,
        });
        let config = build_config(&parser, &env).unwrap();
        assert_eq!(config.default_provider.as_str(), "anthropic");
        assert_eq!(config.default_model.as_str(), "claude-opus-4-6");
    }
}

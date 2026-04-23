use std::fmt;
use std::path::Path;

use confique::Config;
use trogon_service_config::{NatsArgs, NatsConfigSection, load_config, resolve_nats};

#[derive(Debug)]
pub enum ConfigError {
    Load(confique::Error),
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Load(error) => write!(f, "failed to load config: {error}"),
        }
    }
}

impl std::error::Error for ConfigError {}

#[derive(Config)]
struct CronServiceConfig {
    #[config(nested)]
    nats: NatsConfigSection,
}

pub struct ResolvedConfig {
    pub nats: trogon_nats::NatsConfig,
}

#[cfg(test)]
pub fn load(config_path: Option<&Path>) -> Result<ResolvedConfig, ConfigError> {
    load_with_overrides(config_path, &NatsArgs::default())
}

pub fn load_with_overrides(
    config_path: Option<&Path>,
    nats_overrides: &NatsArgs,
) -> Result<ResolvedConfig, ConfigError> {
    let config = load_config::<CronServiceConfig>(config_path).map_err(ConfigError::Load)?;
    Ok(ResolvedConfig {
        nats: resolve_nats(&config.nats, nats_overrides),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use trogon_nats::NatsAuth;

    #[test]
    fn load_uses_nats_defaults() {
        let file = tempfile::Builder::new().suffix(".toml").tempfile().unwrap();
        std::fs::write(file.path(), "").unwrap();

        let resolved = load(Some(file.path())).unwrap();

        assert_eq!(resolved.nats.servers, vec!["localhost:4222"]);
        assert!(matches!(resolved.nats.auth, NatsAuth::None));
    }

    #[test]
    fn load_with_overrides_prefers_cli_nats_values() {
        let file = tempfile::Builder::new().suffix(".toml").tempfile().unwrap();
        std::fs::write(file.path(), "[nats]\nurl = 'file:4222'\ntoken = 'file-token'\n").unwrap();

        let resolved = load_with_overrides(
            Some(file.path()),
            &NatsArgs {
                nats_url: Some("override:4222".to_string()),
                nats_token: Some("override-token".to_string()),
                ..Default::default()
            },
        )
        .unwrap();

        assert_eq!(resolved.nats.servers, vec!["override:4222"]);
        assert!(matches!(resolved.nats.auth, NatsAuth::Token(ref token) if token == "override-token"));
    }
}

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
}

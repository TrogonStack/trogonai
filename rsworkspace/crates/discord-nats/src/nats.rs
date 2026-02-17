//! NATS connection management for Discord integration

use async_nats::Client;
use tracing::{info, warn};

use crate::config::NatsConfig;
use crate::error::{Error, Result};

/// Connect to NATS server(s)
pub async fn connect(config: &NatsConfig) -> Result<Client> {
    info!("Connecting to NATS servers: {:?}", config.servers);

    let mut opts = async_nats::ConnectOptions::new();

    opts = opts.name("discord-bot");

    if let Some(ref creds_file) = config.credentials_file {
        opts = opts
            .credentials_file(creds_file)
            .await
            .map_err(|e| Error::Connection(format!("Failed to load credentials: {}", e)))?;
    } else if let (Some(ref username), Some(ref password)) = (&config.username, &config.password) {
        opts = opts.user_and_password(username.clone(), password.clone());
    }

    opts = opts
        .event_callback(|event| async move {
            match event {
                async_nats::Event::Connected => info!("Connected to NATS"),
                async_nats::Event::Disconnected => warn!("Disconnected from NATS"),
                async_nats::Event::ClientError(e) => warn!("NATS client error: {}", e),
                _ => {}
            }
        })
        .retry_on_initial_connect()
        .max_reconnects(None);

    let servers: Vec<&str> = config.servers.iter().map(|s| s.as_str()).collect();
    let client = opts
        .connect(servers)
        .await
        .map_err(|e| Error::Connection(format!("Failed to connect to NATS: {}", e)))?;

    info!("Successfully connected to NATS");
    Ok(client)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = NatsConfig::default();
        assert_eq!(config.servers, vec!["localhost:4222"]);
        assert_eq!(config.prefix, "prod");
    }

    #[test]
    fn test_config_from_url() {
        let config = NatsConfig::from_url("nats://localhost:4222,nats://localhost:4223", "dev");
        assert_eq!(config.servers.len(), 2);
        assert_eq!(config.prefix, "dev");
    }
}

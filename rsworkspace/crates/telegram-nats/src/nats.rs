//! NATS connection management

use async_nats::Client;
use tracing::{debug, info, warn};

use crate::config::NatsConfig;
use crate::error::{Error, Result};

/// Connect to NATS server
pub async fn connect(config: &NatsConfig) -> Result<Client> {
    info!("Connecting to NATS servers: {:?}", config.servers);

    let mut opts = async_nats::ConnectOptions::new();

    // Set name for client identification
    opts = opts.name("telegram-bot");

    // Configure authentication
    if let Some(ref creds_file) = config.credentials_file {
        debug!("Using credentials file: {}", creds_file);
        opts = opts
            .credentials_file(creds_file)
            .await
            .map_err(|e| Error::Connection(format!("Failed to load credentials: {}", e)))?;
    } else if let (Some(ref username), Some(ref password)) = (&config.username, &config.password) {
        debug!("Using username/password authentication");
        opts = opts.user_and_password(username.clone(), password.clone());
    }

    // Configure reconnect handling
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
        .max_reconnects(None); // Infinite reconnects

    // Connect to servers
    let servers: Vec<&str> = config.servers.iter().map(|s| s.as_str()).collect();
    let client = opts
        .connect(servers)
        .await
        .map_err(|e| Error::Connection(format!("Failed to connect to NATS: {}", e)))?;

    info!("Successfully connected to NATS");
    Ok(client)
}

/// Initialize JetStream for the given client
pub async fn jetstream(client: &Client) -> async_nats::jetstream::Context {
    async_nats::jetstream::new(client.clone())
}

/// Create or update JetStream stream for Telegram events
pub async fn setup_event_stream(js: &async_nats::jetstream::Context, prefix: &str) -> Result<()> {
    let stream_name = format!("telegram_events_{}", prefix);
    let subject_pattern = format!("telegram.{}.bot.>", prefix);

    info!("Setting up JetStream stream: {}", stream_name);

    // Create stream config
    let stream_config = async_nats::jetstream::stream::Config {
        name: stream_name.clone(),
        subjects: vec![subject_pattern],
        max_age: std::time::Duration::from_secs(30 * 24 * 60 * 60), // 30 days
        storage: async_nats::jetstream::stream::StorageType::File,
        retention: async_nats::jetstream::stream::RetentionPolicy::Limits,
        ..Default::default()
    };

    // Try to create or update stream
    match js.get_or_create_stream(stream_config).await {
        Ok(_) => {
            info!("JetStream stream {} ready", stream_name);
            Ok(())
        }
        Err(e) => Err(Error::Other(anyhow::anyhow!(
            "Failed to create stream: {}",
            e
        ))),
    }
}

/// Create or update JetStream KV bucket for session state
pub async fn setup_session_kv(
    js: &async_nats::jetstream::Context,
    prefix: &str,
) -> Result<async_nats::jetstream::kv::Store> {
    let bucket_name = format!("telegram_sessions_{}", prefix);

    info!("Setting up JetStream KV bucket: {}", bucket_name);

    // Create KV bucket config
    let kv_config = async_nats::jetstream::kv::Config {
        bucket: bucket_name.clone(),
        history: 10,
        storage: async_nats::jetstream::stream::StorageType::File,
        ..Default::default()
    };

    // Try to create or get KV bucket
    match js.create_key_value(kv_config).await {
        Ok(kv) => {
            info!("JetStream KV bucket {} ready", bucket_name);
            Ok(kv)
        }
        Err(e) => {
            // If bucket already exists, try to get it
            match js.get_key_value(&bucket_name).await {
                Ok(kv) => {
                    info!("Using existing JetStream KV bucket {}", bucket_name);
                    Ok(kv)
                }
                Err(_) => Err(Error::Other(anyhow::anyhow!(
                    "Failed to create or get KV bucket: {}",
                    e
                ))),
            }
        }
    }
}

/// Create or update JetStream stream for agent commands (at-least-once delivery)
pub async fn setup_agent_stream(js: &async_nats::jetstream::Context, prefix: &str) -> Result<()> {
    let stream_name = format!("telegram_commands_{}", prefix);
    let subject_pattern = format!("telegram.{}.agent.>", prefix);

    info!("Setting up agent command stream: {}", stream_name);

    let stream_config = async_nats::jetstream::stream::Config {
        name: stream_name.clone(),
        subjects: vec![subject_pattern],
        storage: async_nats::jetstream::stream::StorageType::File,
        retention: async_nats::jetstream::stream::RetentionPolicy::WorkQueue,
        ..Default::default()
    };

    match js.get_or_create_stream(stream_config).await {
        Ok(_) => {
            info!("Agent command stream {} ready", stream_name);
            Ok(())
        }
        Err(e) => Err(Error::Other(anyhow::anyhow!(
            "Failed to create agent stream: {}",
            e
        ))),
    }
}

/// Create or get JetStream KV bucket for deduplication (24h TTL)
pub async fn setup_dedup_kv(
    js: &async_nats::jetstream::Context,
    prefix: &str,
) -> Result<async_nats::jetstream::kv::Store> {
    let bucket_name = format!("telegram_dedup_{}", prefix);

    info!("Setting up dedup KV bucket: {}", bucket_name);

    let kv_config = async_nats::jetstream::kv::Config {
        bucket: bucket_name.clone(),
        max_age: std::time::Duration::from_secs(24 * 60 * 60),
        storage: async_nats::jetstream::stream::StorageType::File,
        ..Default::default()
    };

    match js.create_key_value(kv_config).await {
        Ok(kv) => {
            info!("Dedup KV bucket {} ready", bucket_name);
            Ok(kv)
        }
        Err(_) => match js.get_key_value(&bucket_name).await {
            Ok(kv) => {
                info!("Using existing dedup KV bucket {}", bucket_name);
                Ok(kv)
            }
            Err(e) => Err(Error::Other(anyhow::anyhow!(
                "Failed to create or get dedup KV bucket: {}",
                e
            ))),
        },
    }
}

/// Create a durable pull consumer for inbound bot events (Telegram → agent)
///
/// Reads from the `telegram_events_{prefix}` stream with at-least-once delivery.
/// The caller must `ack()` each message after processing.
pub async fn create_inbound_consumer(
    js: &async_nats::jetstream::Context,
    prefix: &str,
    consumer_name: &str,
) -> Result<async_nats::jetstream::consumer::Consumer<async_nats::jetstream::consumer::pull::Config>>
{
    let stream_name = format!("telegram_events_{}", prefix);

    info!(
        "Creating inbound consumer '{}' on stream '{}'",
        consumer_name, stream_name
    );

    let consumer_config = async_nats::jetstream::consumer::pull::Config {
        durable_name: Some(consumer_name.to_string()),
        ack_wait: std::time::Duration::from_secs(30),
        max_deliver: 5,
        ..Default::default()
    };

    let stream = js
        .get_stream(&stream_name)
        .await
        .map_err(|e| Error::Other(anyhow::anyhow!("Failed to get inbound stream: {}", e)))?;

    stream
        .get_or_create_consumer(consumer_name, consumer_config)
        .await
        .map_err(|e| Error::Other(anyhow::anyhow!("Failed to create inbound consumer: {}", e)))
}

/// Create a durable pull consumer for outbound agent commands
pub async fn create_outbound_consumer(
    js: &async_nats::jetstream::Context,
    prefix: &str,
) -> Result<async_nats::jetstream::consumer::Consumer<async_nats::jetstream::consumer::pull::Config>>
{
    let stream_name = format!("telegram_commands_{}", prefix);
    let consumer_name = format!("outbound-{}", prefix);

    info!(
        "Creating outbound consumer '{}' on stream '{}'",
        consumer_name, stream_name
    );

    let consumer_config = async_nats::jetstream::consumer::pull::Config {
        durable_name: Some(consumer_name.clone()),
        ack_wait: std::time::Duration::from_secs(30),
        max_deliver: 5,
        ..Default::default()
    };

    let stream = js
        .get_stream(&stream_name)
        .await
        .map_err(|e| Error::Other(anyhow::anyhow!("Failed to get stream: {}", e)))?;

    stream
        .get_or_create_consumer(&consumer_name, consumer_config)
        .await
        .map_err(|e| Error::Other(anyhow::anyhow!("Failed to create consumer: {}", e)))
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

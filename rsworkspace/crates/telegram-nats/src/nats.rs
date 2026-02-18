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

/// Create a durable pull consumer for error events only (`bot.error.command`).
///
/// Unlike `create_inbound_consumer`, this consumer has a `filter_subject` so it
/// only receives messages published to `telegram.{prefix}.bot.error.command`,
/// not all events in the stream.  This avoids noisy deserialization failures
/// when trying to parse regular text/photo/command events as `CommandErrorEvent`.
pub async fn create_error_consumer(
    js: &async_nats::jetstream::Context,
    prefix: &str,
    consumer_name: &str,
) -> Result<async_nats::jetstream::consumer::Consumer<async_nats::jetstream::consumer::pull::Config>>
{
    let stream_name = format!("telegram_events_{}", prefix);
    let filter = format!("telegram.{}.bot.error.command", prefix);

    info!(
        "Creating error consumer '{}' on stream '{}' (filter: {})",
        consumer_name, stream_name, filter
    );

    let consumer_config = async_nats::jetstream::consumer::pull::Config {
        durable_name: Some(consumer_name.to_string()),
        filter_subject: filter,
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
        .map_err(|e| Error::Other(anyhow::anyhow!("Failed to create error consumer: {}", e)))
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

    // ── Issues 1 + 3: JetStream durable delivery semantics ───────────────────

    const NATS_URL: &str = "nats://localhost:14222";

    async fn try_connect() -> Option<async_nats::Client> {
        async_nats::connect(NATS_URL).await.ok()
    }

    /// Error consumer filter — only error events reach the consumer:
    /// Publishing a regular text event AND an error event to the stream must
    /// result in the error consumer receiving ONLY the error event.
    #[tokio::test]
    async fn test_error_consumer_only_receives_error_events() {
        let Some(client) = try_connect().await else {
            eprintln!("SKIP: NATS not available");
            return;
        };
        let js = async_nats::jetstream::new(client.clone());
        let prefix = format!("errconsumer-{}", uuid::Uuid::new_v4().simple());

        setup_event_stream(&js, &prefix).await.unwrap();

        // Publish a regular text event — the error consumer must NOT see this
        let text_subject = crate::subjects::bot::message_text(&prefix);
        client
            .publish(text_subject, b"text-payload".as_ref().into())
            .await
            .unwrap();

        // Publish an error event — the error consumer MUST see this
        let error_subject = crate::subjects::bot::command_error(&prefix);
        client
            .publish(error_subject, b"error-payload".as_ref().into())
            .await
            .unwrap();

        let consumer_name = format!("err-filter-{}", uuid::Uuid::new_v4().simple());
        let consumer = create_error_consumer(&js, &prefix, &consumer_name)
            .await
            .unwrap();
        let mut messages = consumer.messages().await.unwrap();

        let msg = tokio::time::timeout(
            std::time::Duration::from_secs(3),
            futures::StreamExt::next(&mut messages),
        )
        .await
        .expect("timed out — error event not received")
        .unwrap()
        .unwrap();

        assert_eq!(
            &msg.payload[..],
            b"error-payload",
            "error consumer must only receive error events, not text events"
        );
        msg.ack().await.unwrap();

        // No second message should arrive (text event filtered out)
        let second = tokio::time::timeout(
            std::time::Duration::from_millis(500),
            futures::StreamExt::next(&mut messages),
        )
        .await;
        assert!(
            second.is_err(),
            "error consumer must not receive the text event"
        );
    }

    /// Issue 3 – Inbound at-least-once:
    /// A message published BEFORE any consumer is active must still be
    /// retrievable once a durable pull consumer is created later.
    #[tokio::test]
    async fn test_inbound_message_persists_without_active_consumer() {
        let Some(client) = try_connect().await else {
            eprintln!("SKIP: NATS not available");
            return;
        };
        let js = async_nats::jetstream::new(client.clone());
        let prefix = format!("persist-{}", uuid::Uuid::new_v4().simple());

        setup_event_stream(&js, &prefix).await.unwrap();

        // Publish BEFORE creating any consumer — stream must buffer the message
        let subject = crate::subjects::bot::message_text(&prefix);
        client
            .publish(subject, b"at-least-once-payload".as_ref().into())
            .await
            .unwrap();

        // Brief pause (no consumer active during this window)
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        // Now create the consumer — message must still be available
        let consumer_name = format!("persist-test-{}", uuid::Uuid::new_v4().simple());
        let consumer = create_inbound_consumer(&js, &prefix, &consumer_name)
            .await
            .unwrap();
        let mut messages = consumer.messages().await.unwrap();

        let msg = tokio::time::timeout(
            std::time::Duration::from_secs(3),
            futures::StreamExt::next(&mut messages),
        )
        .await
        .expect("timed out — message was not retained in JetStream stream")
        .expect("stream closed unexpectedly")
        .unwrap();

        assert_eq!(&msg.payload[..], b"at-least-once-payload");
        msg.ack().await.unwrap();
    }

    /// Issue 1 – Durable consumer redelivers unacked message:
    /// If a consumer receives a message but does NOT ack within ack_wait,
    /// JetStream must redeliver it (at-least-once guarantee on crash).
    #[tokio::test]
    async fn test_durable_consumer_redelivers_unacked_message() {
        let Some(client) = try_connect().await else {
            eprintln!("SKIP: NATS not available");
            return;
        };
        let js = async_nats::jetstream::new(client.clone());
        let prefix = format!("redeliver-{}", uuid::Uuid::new_v4().simple());

        setup_event_stream(&js, &prefix).await.unwrap();

        let subject = crate::subjects::bot::message_text(&prefix);
        client
            .publish(subject, b"redeliver-payload".as_ref().into())
            .await
            .unwrap();

        // Create consumer with a short ack_wait so the test doesn't take too long
        let stream_name = format!("telegram_events_{}", prefix);
        let stream = js.get_stream(&stream_name).await.unwrap();
        let consumer_name = format!("redeliver-{}", uuid::Uuid::new_v4().simple());
        let config = async_nats::jetstream::consumer::pull::Config {
            durable_name: Some(consumer_name.clone()),
            ack_wait: std::time::Duration::from_secs(2),
            max_deliver: 5,
            ..Default::default()
        };
        let consumer = stream
            .get_or_create_consumer(&consumer_name, config)
            .await
            .unwrap();
        let mut messages = consumer.messages().await.unwrap();

        // First delivery — do NOT ack (simulates consumer crash)
        let first = tokio::time::timeout(
            std::time::Duration::from_secs(3),
            futures::StreamExt::next(&mut messages),
        )
        .await
        .expect("timed out on first delivery")
        .unwrap()
        .unwrap();

        let first_seq = first.info().unwrap().stream_sequence;
        // Intentionally drop without acking — triggers redelivery after ack_wait
        drop(first);

        // Wait past ack_wait (2 s) so JetStream redelivers
        tokio::time::sleep(std::time::Duration::from_secs(3)).await;

        // Second delivery must be the same message
        let second = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            futures::StreamExt::next(&mut messages),
        )
        .await
        .expect("timed out — JetStream did not redeliver unacked message")
        .unwrap()
        .unwrap();

        assert_eq!(
            second.info().unwrap().stream_sequence,
            first_seq,
            "redelivered message must have the same stream_sequence"
        );
        assert!(
            second.info().unwrap().delivered >= 2,
            "delivered count must be >= 2 on redelivery"
        );
        second.ack().await.unwrap();
    }
}

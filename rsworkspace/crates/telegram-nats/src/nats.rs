use tracing::info;

use crate::error::{Error, Result};

pub async fn setup_event_stream(js: &async_nats::jetstream::Context, prefix: &str) -> Result<()> {
    let stream_name = format!("telegram_events_{}", prefix);
    let subject_pattern = format!("telegram.{}.bot.>", prefix);

    info!("Setting up JetStream stream: {}", stream_name);

    let stream_config = async_nats::jetstream::stream::Config {
        name: stream_name.clone(),
        subjects: vec![subject_pattern],
        max_age: std::time::Duration::from_secs(30 * 24 * 60 * 60),
        storage: async_nats::jetstream::stream::StorageType::File,
        retention: async_nats::jetstream::stream::RetentionPolicy::Limits,
        ..Default::default()
    };

    js.get_or_create_stream(stream_config)
        .await
        .map_err(|e| Error::Setup(format!("Failed to create stream {stream_name}: {e}")))?;

    info!("JetStream stream {} ready", stream_name);
    Ok(())
}

pub async fn setup_session_kv(
    js: &async_nats::jetstream::Context,
    prefix: &str,
) -> Result<async_nats::jetstream::kv::Store> {
    let bucket_name = format!("telegram_sessions_{}", prefix);

    info!("Setting up JetStream KV bucket: {}", bucket_name);

    let kv_config = async_nats::jetstream::kv::Config {
        bucket: bucket_name.clone(),
        history: 10,
        storage: async_nats::jetstream::stream::StorageType::File,
        ..Default::default()
    };

    match js.create_key_value(kv_config).await {
        Ok(kv) => {
            info!("JetStream KV bucket {} ready", bucket_name);
            Ok(kv)
        }
        Err(_) => match js.get_key_value(&bucket_name).await {
            Ok(kv) => {
                info!("Using existing JetStream KV bucket {}", bucket_name);
                Ok(kv)
            }
            Err(e) => Err(Error::Setup(format!(
                "Failed to create or get KV bucket {bucket_name}: {e}"
            ))),
        },
    }
}

pub async fn setup_agent_stream(
    js: &async_nats::jetstream::Context,
    prefix: &str,
) -> Result<()> {
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

    js.get_or_create_stream(stream_config)
        .await
        .map_err(|e| Error::Setup(format!("Failed to create agent stream {stream_name}: {e}")))?;

    info!("Agent command stream {} ready", stream_name);
    Ok(())
}

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
            Err(e) => Err(Error::Setup(format!(
                "Failed to create or get dedup KV bucket {bucket_name}: {e}"
            ))),
        },
    }
}

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
        .map_err(|e| Error::Setup(format!("Failed to get inbound stream: {e}")))?;

    stream
        .get_or_create_consumer(consumer_name, consumer_config)
        .await
        .map_err(|e| Error::Setup(format!("Failed to create inbound consumer: {e}")))
}

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
        .map_err(|e| Error::Setup(format!("Failed to get inbound stream: {e}")))?;

    stream
        .get_or_create_consumer(consumer_name, consumer_config)
        .await
        .map_err(|e| Error::Setup(format!("Failed to create error consumer: {e}")))
}

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
        .map_err(|e| Error::Setup(format!("Failed to get stream: {e}")))?;

    stream
        .get_or_create_consumer(&consumer_name, consumer_config)
        .await
        .map_err(|e| Error::Setup(format!("Failed to create consumer: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    const NATS_URL: &str = "nats://localhost:14222";

    async fn try_connect() -> Option<async_nats::Client> {
        async_nats::connect(NATS_URL).await.ok()
    }

    #[tokio::test]
    async fn test_error_consumer_only_receives_error_events() {
        let Some(client) = try_connect().await else {
            eprintln!("SKIP: NATS not available");
            return;
        };
        let js = async_nats::jetstream::new(client.clone());
        let prefix = format!("errconsumer-{}", uuid::Uuid::new_v4().simple());

        setup_event_stream(&js, &prefix).await.unwrap();

        let text_subject = crate::subjects::bot::message_text(&prefix);
        client
            .publish(text_subject, b"text-payload".as_ref().into())
            .await
            .unwrap();

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

    #[tokio::test]
    async fn test_inbound_message_persists_without_active_consumer() {
        let Some(client) = try_connect().await else {
            eprintln!("SKIP: NATS not available");
            return;
        };
        let js = async_nats::jetstream::new(client.clone());
        let prefix = format!("persist-{}", uuid::Uuid::new_v4().simple());

        setup_event_stream(&js, &prefix).await.unwrap();

        let subject = crate::subjects::bot::message_text(&prefix);
        client
            .publish(subject, b"at-least-once-payload".as_ref().into())
            .await
            .unwrap();

        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

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

        let first = tokio::time::timeout(
            std::time::Duration::from_secs(3),
            futures::StreamExt::next(&mut messages),
        )
        .await
        .expect("timed out on first delivery")
        .unwrap()
        .unwrap();

        let first_seq = first.info().unwrap().stream_sequence;
        drop(first);

        tokio::time::sleep(std::time::Duration::from_secs(3)).await;

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

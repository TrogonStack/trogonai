use std::error::Error;
use std::time::Duration;

use acp_nats::acp_prefix::AcpPrefix;
use acp_nats::jetstream::provision::provision_streams;
use acp_nats::nats::AcpStream;
use async_nats::jetstream::consumer::{AckPolicy, pull};
use async_nats::jetstream::stream::StorageType;
use bytes::Bytes;
use trogon_nats::jetstream::{JetStreamContext, MockJetStreamContext, NatsJetStreamClient};
use trogon_nats::test_support::JetStreamTestServer;

const ADDED_METHODS: [&str; 4] = ["set_title", "status", "unarchive", "archive"];
const OLD_METHODS: [&str; 9] = [
    "prompt",
    "cancel",
    "load",
    "set_mode",
    "set_config_option",
    "fork",
    "resume",
    "close",
    "delete",
];

#[tokio::test]
async fn existing_commands_stream_upgrades_without_losing_retained_state() -> Result<(), Box<dyn Error + Send + Sync>> {
    let broker = JetStreamTestServer::start().await;
    let js = broker.jetstream().await;
    let adapter = NatsJetStreamClient::new(js.clone());
    let fresh_prefix = AcpPrefix::new("fresh_acp")?;
    provision_streams(&adapter, &fresh_prefix).await?;
    let fresh = js.get_stream(AcpStream::Commands.stream_name(&fresh_prefix)).await?;
    for method in ADDED_METHODS {
        assert!(
            fresh
                .cached_info()
                .config
                .subjects
                .contains(&format!("fresh_acp.v1.session.*.agent.{method}")),
            "fresh provisioning must capture {method}"
        );
    }

    let prefix = AcpPrefix::new("existing_acp")?;
    let mut old_config = AcpStream::Commands.config(&prefix);
    old_config.subjects = OLD_METHODS
        .map(|method| format!("existing_acp.v1.session.*.agent.{method}"))
        .into();
    old_config.storage = StorageType::Memory;
    old_config.max_bytes = 1_048_576;
    old_config.max_messages = 100;
    old_config.max_age = Duration::from_secs(600);
    old_config.duplicate_window = Duration::from_secs(5);
    old_config.description = Some("operator-managed commands stream".to_owned());
    let existing = js.create_stream(old_config.clone()).await?;
    let stream_created = existing.cached_info().created;
    let prompt_subject = "existing_acp.v1.session.retained.agent.prompt";
    let consumer = existing
        .create_consumer(pull::Config {
            durable_name: Some("retained_commands".to_owned()),
            filter_subject: prompt_subject.to_owned(),
            ack_policy: AckPolicy::Explicit,
            ..Default::default()
        })
        .await?;
    let consumer_created = consumer.cached_info().created;
    let payload = Bytes::from_static(b"retained prompt");
    let retained = js.publish(prompt_subject, payload.clone()).await?.await?;

    provision_streams(&adapter, &prefix).await?;

    let upgraded = js.get_stream(AcpStream::Commands.stream_name(&prefix)).await?;
    let info = upgraded.cached_info();
    assert_eq!(info.created, stream_created, "the stream must not be recreated");
    assert_eq!(info.state.messages, 1);
    assert_eq!(info.state.consumer_count, 1);
    assert_eq!(info.config.storage, old_config.storage);
    assert_eq!(info.config.max_bytes, old_config.max_bytes);
    assert_eq!(info.config.max_messages, old_config.max_messages);
    assert_eq!(info.config.max_age, old_config.max_age);
    assert_eq!(info.config.duplicate_window, old_config.duplicate_window);
    assert_eq!(info.config.description, old_config.description);
    let stored = upgraded.get_raw_message(retained.sequence).await?;
    assert_eq!(stored.payload, payload);
    let durable = upgraded.get_consumer::<pull::Config>("retained_commands").await?;
    assert_eq!(durable.cached_info().created, consumer_created);
    assert_eq!(durable.cached_info().config.filter_subject, prompt_subject);
    assert_eq!(durable.cached_info().config.ack_policy, AckPolicy::Explicit);

    let missing: Vec<_> = ADDED_METHODS
        .into_iter()
        .filter(|method| {
            !info
                .config
                .subjects
                .contains(&format!("existing_acp.v1.session.*.agent.{method}"))
        })
        .collect();
    assert!(
        missing.is_empty(),
        "existing COMMANDS stream is missing new filters: {missing:?}"
    );

    for method in ADDED_METHODS {
        let subject = format!("existing_acp.v1.session.retained.agent.{method}");
        let ack = js.publish(subject, Bytes::from_static(b"new command")).await?.await?;
        assert_eq!(ack.stream, AcpStream::Commands.stream_name(&prefix));
    }
    Ok(())
}

#[tokio::test]
async fn repeated_provisioning_preserves_operator_configuration_without_another_write()
-> Result<(), Box<dyn Error + Send + Sync>> {
    let context = MockJetStreamContext::new();
    let prefix = AcpPrefix::new("operator_acp")?;
    let mut old = AcpStream::Commands.config(&prefix);
    old.subjects = OLD_METHODS
        .map(|method| format!("operator_acp.v1.session.*.agent.{method}"))
        .into();
    old.storage = StorageType::Memory;
    old.max_bytes = 1_048_576;
    old.max_messages = 100;
    old.max_age = Duration::from_secs(600);
    old.duplicate_window = Duration::from_secs(5);
    old.description = Some("operator-managed commands stream".to_owned());
    context.get_or_create_stream(old.clone()).await?;
    assert_eq!(context.stream_writes(), 1);

    provision_streams(&context, &prefix).await?;
    let configured = context.created_streams();
    assert_eq!(configured.len(), 5);
    let commands = configured.iter().find(|config| config.name == old.name);
    let mut expected = old;
    expected.subjects = AcpStream::Commands.config(&prefix).subjects;
    assert_eq!(commands, Some(&expected));
    assert_eq!(context.stream_writes(), 6);

    provision_streams(&context, &prefix).await?;
    assert_eq!(context.created_streams(), configured);
    assert_eq!(context.stream_writes(), 6);
    Ok(())
}

#[tokio::test]
async fn overlapping_subject_claim_refuses_reconciliation_without_recreating_the_stream()
-> Result<(), Box<dyn Error + Send + Sync>> {
    let broker = JetStreamTestServer::start().await;
    let jetstream = broker.jetstream().await;
    let adapter = NatsJetStreamClient::new(jetstream.clone());
    let prefix = AcpPrefix::new("conflicting_acp")?;
    let mut config = AcpStream::Commands.config(&prefix);
    config.subjects = OLD_METHODS
        .map(|method| format!("conflicting_acp.v1.session.*.agent.{method}"))
        .into();
    config.storage = StorageType::Memory;
    let stream = jetstream.create_stream(config).await?;
    let created = stream.cached_info().created;
    let original = stream.cached_info().config.clone();
    let conflicting = jetstream
        .create_stream(async_nats::jetstream::stream::Config {
            name: "CLAIMED_ARCHIVE".to_owned(),
            subjects: vec!["conflicting_acp.v1.session.*.agent.archive".to_owned()],
            storage: StorageType::Memory,
            ..Default::default()
        })
        .await?;
    let conflicting_created = conflicting.cached_info().created;
    let conflicting_config = conflicting.cached_info().config.clone();

    let error = provision_streams(&adapter, &prefix).await.err();
    assert!(error.as_ref().and_then(|error| error.source()).is_some());
    let source = error
        .as_ref()
        .and_then(|error| error.source())
        .and_then(|source| source.downcast_ref::<async_nats::jetstream::context::CreateStreamError>());
    assert!(matches!(
        source.map(|source| source.kind()),
        Some(async_nats::jetstream::context::CreateStreamErrorKind::JetStream(error))
            if error.error_code() == async_nats::jetstream::ErrorCode::STREAM_SUBJECT_OVERLAP
    ));
    let untouched = jetstream.get_stream(AcpStream::Commands.stream_name(&prefix)).await?;
    assert_eq!(untouched.cached_info().created, created);
    assert_eq!(untouched.cached_info().config, original);
    let untouched_claim = jetstream.get_stream("CLAIMED_ARCHIVE").await?;
    assert_eq!(untouched_claim.cached_info().created, conflicting_created);
    assert_eq!(untouched_claim.cached_info().config, conflicting_config);
    Ok(())
}

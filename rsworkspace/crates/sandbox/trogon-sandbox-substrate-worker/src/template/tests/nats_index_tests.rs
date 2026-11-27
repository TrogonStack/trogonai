use std::error::Error;

use async_nats::jetstream::kv;
use trogon_nats::test_support::JetStreamTestServer;

use crate::template::{NatsTemplateIndex, TEMPLATE_KV_BUCKET, TemplateIndex, TemplatePhase, TemplateRecord};

type TestResult = Result<(), Box<dyn Error>>;

fn record(phase: TemplatePhase) -> TemplateRecord {
    TemplateRecord {
        atespace: "tenant-a".into(),
        name: "tpl-1".into(),
        phase,
        created_at: 42,
        message: Some("why".into()),
    }
}

#[tokio::test]
async fn create_elects_one_writer_and_get_reads_it_back() -> TestResult {
    let server = JetStreamTestServer::start().await;
    let index = NatsTemplateIndex::new(server.jetstream().await).await?;

    assert!(index.get("k").await?.is_none());
    assert!(index.create("k", &record(TemplatePhase::Building)).await?);
    assert!(
        !index.create("k", &record(TemplatePhase::Ready)).await?,
        "the second create must lose without overwriting"
    );

    let stored = index.get("k").await?.ok_or("record")?;
    assert_eq!(stored.phase, TemplatePhase::Building);
    assert_eq!(stored.created_at, 42);
    assert_eq!(stored.message.as_deref(), Some("why"));
    Ok(())
}

#[tokio::test]
async fn put_overwrites_and_delete_removes() -> TestResult {
    let server = JetStreamTestServer::start().await;
    let index = NatsTemplateIndex::new(server.jetstream().await).await?;

    index.put("k", &record(TemplatePhase::Building)).await?;
    index.put("k", &record(TemplatePhase::Ready)).await?;
    assert_eq!(index.get("k").await?.ok_or("record")?.phase, TemplatePhase::Ready);

    index.delete("k").await?;
    assert!(index.get("k").await?.is_none());
    Ok(())
}

#[tokio::test]
async fn a_corrupt_entry_reads_as_absent_so_it_gets_rebuilt() -> TestResult {
    let server = JetStreamTestServer::start().await;
    let jetstream = server.jetstream().await;
    let index = NatsTemplateIndex::new(jetstream.clone()).await?;

    let store = jetstream.get_key_value(TEMPLATE_KV_BUCKET).await?;
    store.put("k", "not json".into()).await?;

    assert!(index.get("k").await?.is_none());
    Ok(())
}

#[tokio::test]
async fn opening_the_index_twice_shares_one_bucket() -> TestResult {
    let server = JetStreamTestServer::start().await;
    let jetstream = server.jetstream().await;
    let first = NatsTemplateIndex::new(jetstream.clone()).await?;
    let second = NatsTemplateIndex::new(jetstream).await?;

    first.put("k", &record(TemplatePhase::Ready)).await?;
    assert_eq!(second.get("k").await?.ok_or("record")?.phase, TemplatePhase::Ready);
    Ok(())
}

#[tokio::test]
async fn every_operation_reports_storage_failure_when_the_bucket_is_gone() -> TestResult {
    let server = JetStreamTestServer::start().await;
    let jetstream = server.jetstream().await;
    let index = NatsTemplateIndex::new(jetstream.clone()).await?;
    jetstream.delete_key_value(TEMPLATE_KV_BUCKET).await?;

    assert!(index.get("k").await.is_err());
    assert!(index.create("k", &record(TemplatePhase::Building)).await.is_err());
    assert!(index.put("k", &record(TemplatePhase::Building)).await.is_err());
    assert!(index.delete("k").await.is_err());
    Ok(())
}

#[tokio::test]
async fn a_conflicting_bucket_configuration_is_a_storage_failure() -> TestResult {
    let server = JetStreamTestServer::start().await;
    let jetstream = server.jetstream().await;
    jetstream
        .create_key_value(kv::Config {
            bucket: TEMPLATE_KV_BUCKET.to_string(),
            storage: async_nats::jetstream::stream::StorageType::Memory,
            ..Default::default()
        })
        .await?;

    // `create_or_update` cannot change the storage backend of an existing bucket.
    let result = NatsTemplateIndex::new(jetstream).await;
    assert!(result.is_err());
    Ok(())
}

use crate::ulid::Ulid;
use async_nats::jetstream::stream::{RawMessageError, RawMessageErrorKind};
use bytes::Bytes;
use serde_json::json;
use trogon_nats::test_support::JetStreamTestServer;
use trogon_sandbox_contract::{NativeSnapshotId, NativeSnapshotRef, ProviderKind, TenantId};
use trogon_sandbox_protocol::tenant_scoped_kv_key;

use super::constants::BUCKET;
use super::store::live_message;
use super::*;
use crate::snapshots::SnapshotTemplateId;

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn tenant(name: &str) -> Result<TenantId, Box<dyn std::error::Error>> {
    Ok(TenantId::new(name)?)
}

fn reference(id: &str) -> Result<NativeSnapshotRef, Box<dyn std::error::Error>> {
    Ok(NativeSnapshotRef {
        provider: ProviderKind::E2b,
        id: NativeSnapshotId::new(id)?,
        expires_at: None,
    })
}

fn template() -> Result<SnapshotTemplateId, Box<dyn std::error::Error>> {
    Ok(SnapshotTemplateId::try_from("tpl-1".to_owned())?)
}

fn capturing(id: &str) -> Result<CustodyRecord, Box<dyn std::error::Error>> {
    Ok(CustodyRecord::capturing(tenant("tenant-a")?, reference(id)?, 5))
}

async fn configured() -> Result<(JetStreamTestServer, SnapshotCustody), Box<dyn std::error::Error>> {
    let server = JetStreamTestServer::start().await;
    let custody = SnapshotCustody::default();
    custody.configure(server.jetstream().await)?;
    Ok((server, custody))
}

#[test]
fn phases_use_snake_case_names() -> TestResult {
    assert_eq!(serde_json::to_string(&CustodyPhase::Capturing)?, "\"capturing\"");
    Ok(())
}

#[test]
fn the_clock_reads_milliseconds_and_saturates() -> TestResult {
    let now = now_ms()?;
    assert!(now > 1_600_000_000_000);
    assert!(after_ms(std::time::Duration::from_secs(10))? >= now + 10_000);
    assert_eq!(after_ms(std::time::Duration::MAX)?, u64::MAX);
    Ok(())
}

#[test]
fn custody_records_need_an_exact_identity() -> TestResult {
    let id = Ulid::generate().to_string();
    capturing(&id)?.validate()?;

    let mut owned = capturing(&id)?;
    owned.phase = CustodyPhase::Owned;
    assert!(matches!(owned.validate(), Err(SnapshotCustodyError::IdentityMissing)));
    owned.template_id = Some(template()?);
    owned.validate()?;
    let mut capturing_with_template = capturing(&id)?;
    capturing_with_template.template_id = Some(template()?);
    assert!(matches!(
        capturing_with_template.validate(),
        Err(SnapshotCustodyError::IdentityMissing)
    ));

    let mut versioned = capturing(&id)?;
    versioned.version = 2;
    assert!(matches!(
        versioned.validate(),
        Err(SnapshotCustodyError::InvalidVersion)
    ));
    assert!(matches!(
        capturing("not-opaque")?.validate(),
        Err(SnapshotCustodyError::InvalidReferenceId)
    ));
    let mut foreign = capturing(&id)?;
    foreign.reference.provider = ProviderKind::Vercel;
    assert!(matches!(foreign.validate(), Err(SnapshotCustodyError::Reference(_))));
    Ok(())
}

#[tokio::test]
async fn custody_requires_a_context_configured_once() -> TestResult {
    let custody = SnapshotCustody::default();
    assert!(matches!(
        custody.create(capturing(&Ulid::generate().to_string())?).await,
        Err(SnapshotCustodyError::NotConfigured)
    ));
    let server = JetStreamTestServer::start().await;
    custody.configure(server.jetstream().await)?;
    assert!(matches!(
        custody.configure(server.jetstream().await),
        Err(SnapshotCustodyError::AlreadyConfigured)
    ));
    Ok(())
}

#[tokio::test]
async fn custody_records_are_created_loaded_and_saved_by_revision() -> TestResult {
    let (_server, custody) = configured().await?;
    let id = Ulid::generate().to_string();
    let tenant = tenant("tenant-a")?;
    let reference = reference(&id)?;
    assert!(custody.load(&tenant, &reference).await?.is_none());

    let created = custody.create(capturing(&id)?).await?;
    assert!(custody.create(capturing(&id)?).await.is_err());
    let loaded = custody.load(&tenant, &reference).await?.ok_or("custody is missing")?;
    assert_eq!(loaded.revision, created.revision);
    assert_eq!(loaded.record.phase, CustodyPhase::Capturing);

    let mut owned = loaded.record;
    owned.phase = CustodyPhase::Owned;
    owned.template_id = Some(template()?);
    let saved = custody.save(owned.clone(), loaded.revision).await?;
    assert!(saved.revision > loaded.revision);
    assert!(custody.save(owned, loaded.revision).await.is_err());
    Ok(())
}

#[tokio::test]
async fn stored_custody_is_checked_when_read() -> TestResult {
    let server = JetStreamTestServer::start().await;
    let context = server.jetstream().await;
    let custody = SnapshotCustody::default();
    custody.configure(context.clone())?;
    custody.create(capturing(&Ulid::generate().to_string())?).await?;
    let bucket = context.get_key_value(BUCKET).await?;
    let id = Ulid::generate().to_string();
    let tenant = tenant("tenant-a")?;
    let reference = reference(&id)?;
    let key = tenant_scoped_kv_key(&tenant, &id);

    bucket.put(&key, Bytes::from_static(b"not json")).await?;
    assert!(matches!(
        custody.load(&tenant, &reference).await,
        Err(SnapshotCustodyError::Decode(_))
    ));
    bucket.put(&key, Bytes::from(vec![b' '; 4097])).await?;
    assert!(matches!(
        custody.load(&tenant, &reference).await,
        Err(SnapshotCustodyError::TooLarge)
    ));
    let foreign = CustodyRecord::capturing(self::tenant("tenant-b")?, self::reference(&id)?, 5);
    bucket.put(&key, Bytes::from(serde_json::to_vec(&foreign)?)).await?;
    assert!(matches!(
        custody.load(&tenant, &reference).await,
        Err(SnapshotCustodyError::IdentityMismatch)
    ));
    bucket.delete(&key).await?;
    assert!(custody.load(&tenant, &reference).await?.is_none());
    Ok(())
}

#[tokio::test]
async fn scans_return_live_records_in_batches_and_wrap() -> TestResult {
    let server = JetStreamTestServer::start().await;
    let context = server.jetstream().await;
    let custody = SnapshotCustody::default();
    custody.configure(context.clone())?;
    assert!(custody.scan().await?.is_empty());

    let first = custody.create(capturing(&Ulid::generate().to_string())?).await?;
    let second = custody.create(capturing(&Ulid::generate().to_string())?).await?;
    let stale = custody.create(capturing(&Ulid::generate().to_string())?).await?;
    let gone = custody.create(capturing(&Ulid::generate().to_string())?).await?;
    let rewritten = custody.save(stale.record.clone(), stale.revision).await?;
    context.get_key_value(BUCKET).await?.delete(gone.record.key()).await?;

    let found = custody.scan().await?;
    let mut revisions: Vec<u64> = found.iter().map(|stored| stored.revision).collect();
    revisions.sort_unstable();
    assert_eq!(revisions, vec![first.revision, second.revision, rewritten.revision]);
    assert_eq!(custody.scan().await?.len(), 3);

    let bucket = context.get_key_value(BUCKET).await?;
    let prefix = format!("$KV.{BUCKET}.");
    context
        .publish(format!("{prefix}raw-text"), Bytes::from_static(b"not json"))
        .await?
        .await?;
    assert!(matches!(custody.scan().await, Err(SnapshotCustodyError::Decode(_))));
    bucket.purge("raw-text").await?;
    context
        .publish(format!("{prefix}raw-large"), Bytes::from(vec![b' '; 4097]))
        .await?
        .await?;
    assert!(matches!(custody.scan().await, Err(SnapshotCustodyError::TooLarge)));
    bucket.purge("raw-large").await?;
    let misplaced = serde_json::to_vec(&capturing(&Ulid::generate().to_string())?)?;
    context
        .publish(format!("{prefix}raw-misplaced"), Bytes::from(misplaced))
        .await?
        .await?;
    assert!(matches!(
        custody.scan().await,
        Err(SnapshotCustodyError::SubjectMismatch)
    ));
    bucket.purge("raw-misplaced").await?;
    assert!(custody.scan().await?.is_empty());
    assert_eq!(custody.scan().await?.len(), 3);
    Ok(())
}

#[tokio::test]
async fn custody_storage_failures_are_reported() -> TestResult {
    let (server, custody) = configured().await?;
    let context = server.jetstream().await;
    let id = Ulid::generate().to_string();
    let stored = custody.create(capturing(&id)?).await?;
    context.delete_key_value(BUCKET).await?;

    assert!(matches!(
        custody.create(capturing(&Ulid::generate().to_string())?).await,
        Err(SnapshotCustodyError::Bucket(_))
    ));
    assert!(matches!(
        custody.save(stored.record, stored.revision).await,
        Err(SnapshotCustodyError::Bucket(_))
    ));
    assert!(matches!(
        custody.load(&tenant("tenant-a")?, &reference(&id)?).await,
        Err(SnapshotCustodyError::Bucket(_))
    ));
    assert!(matches!(custody.scan().await, Err(SnapshotCustodyError::Bucket(_))));
    Ok(())
}

#[test]
fn custody_records_serialize_without_an_absent_template() -> TestResult {
    let value = serde_json::to_value(capturing(&Ulid::generate().to_string())?)?;
    assert_eq!(value["phase"], json!("capturing"));
    assert!(value.get("template_id").is_none());
    Ok(())
}

#[tokio::test]
async fn an_expiring_bucket_is_refused_on_open_and_after_a_change() -> TestResult {
    let server = JetStreamTestServer::start().await;
    let context = server.jetstream().await;
    context
        .create_key_value(async_nats::jetstream::kv::Config {
            bucket: BUCKET.into(),
            history: 1,
            max_age: std::time::Duration::from_secs(600),
            ..Default::default()
        })
        .await?;
    let custody = SnapshotCustody::default();
    custody.configure(context)?;
    assert!(matches!(
        custody.scan().await,
        Err(SnapshotCustodyError::Bucket(crate::CustodyBucketError::NotDurable))
    ));

    let (server, custody) = configured().await?;
    assert!(custody.scan().await?.is_empty());
    let context = server.jetstream().await;
    let mut config = context
        .get_stream(format!("KV_{BUCKET}"))
        .await?
        .cached_info()
        .config
        .clone();
    config.max_age = std::time::Duration::from_secs(600);
    context.update_stream(config).await?;
    assert!(matches!(
        custody.scan().await,
        Err(SnapshotCustodyError::Bucket(crate::CustodyBucketError::NotDurable))
    ));
    Ok(())
}

#[test]
fn a_sequence_holding_nothing_is_skipped_and_any_other_read_failure_is_surfaced() {
    assert!(matches!(
        live_message(Err(RawMessageError::new(RawMessageErrorKind::NoMessageFound))),
        Ok(None)
    ));
    assert!(matches!(
        live_message(Err(RawMessageError::new(RawMessageErrorKind::Other))),
        Err(SnapshotCustodyError::Message(_))
    ));
}

use std::time::Duration;

use trogon_nats::test_support::JetStreamTestServer;
use trogon_sandbox_contract::{CancelOwnedLeaseRequest, LeaseId, SandboxId, TenantId};

use super::*;
use crate::lease_cleanup::constants::{BUCKET, MAX_LEASE_ID_BYTES, MAX_RECORD_BYTES};
use crate::lease_cleanup::receipts::within_limit;

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn request(lease: &str, sandbox: &str) -> Result<CancelOwnedLeaseRequest, Box<dyn std::error::Error>> {
    Ok(CancelOwnedLeaseRequest {
        tenant: TenantId::new("t")?,
        lease_id: LeaseId::new(lease),
        sandbox_id: SandboxId::new(sandbox)?,
    })
}

async fn configured() -> Result<(JetStreamTestServer, LeaseCleanupReceipts), Box<dyn std::error::Error>> {
    let server = JetStreamTestServer::start().await;
    let receipts = LeaseCleanupReceipts::default();
    receipts.configure(server.jetstream().await)?;
    Ok((server, receipts))
}

#[test]
fn a_request_needs_a_bounded_lease_and_a_canonical_sandbox() -> TestResult {
    assert!(validate_request_for(&request("l1", "sbx1")?).is_ok());
    assert!(matches!(
        validate_request_for(&request("", "sbx1")?),
        Err(LeaseCleanupError::UnboundedOwner)
    ));
    assert!(matches!(
        validate_request_for(&request(&"a".repeat(MAX_LEASE_ID_BYTES + 1), "sbx1")?),
        Err(LeaseCleanupError::UnboundedOwner)
    ));
    assert!(matches!(
        validate_request_for(&request("a\nb", "sbx1")?),
        Err(LeaseCleanupError::UnboundedOwner)
    ));
    assert!(matches!(
        validate_request_for(&request("l1", "a/b")?),
        Err(LeaseCleanupError::SandboxId(_))
    ));
    Ok(())
}

fn validate_request_for(request: &CancelOwnedLeaseRequest) -> Result<(), LeaseCleanupError> {
    super::request_check::validate_request(request)
}

#[tokio::test]
async fn an_unconfigured_journal_refuses_work() -> TestResult {
    let receipts = LeaseCleanupReceipts::default();
    assert!(!receipts.is_configured());
    assert!(matches!(
        receipts.load(&request("l1", "sbx1")?).await,
        Err(LeaseCleanupError::NotConfigured)
    ));
    Ok(())
}

#[tokio::test]
async fn a_journal_configures_once() -> TestResult {
    let (server, receipts) = configured().await?;
    assert!(receipts.is_configured());
    assert!(matches!(
        receipts.configure(server.jetstream().await),
        Err(LeaseCleanupError::AlreadyConfigured)
    ));
    Ok(())
}

#[tokio::test]
async fn a_cleanup_is_begun_claimed_and_confirmed() -> TestResult {
    let (_server, receipts) = configured().await?;
    let request = request("l1", "sbx1")?;
    assert!(receipts.load(&request).await?.is_none());
    let begun = receipts.begin(&request).await?;
    assert!(!begun.absent());
    let again = receipts.begin(&request).await?;
    assert!(!again.absent());
    let claimed = receipts.claim(&request, again, Duration::from_secs(60)).await?;
    let held = receipts.load(&request).await?.ok_or("receipt")?;
    assert!(matches!(
        receipts.claim(&request, held, Duration::from_secs(60)).await,
        Err(LeaseCleanupError::InProgress)
    ));
    receipts.release(&request, claimed).await?;
    let released = receipts.load(&request).await?.ok_or("receipt")?;
    let claimed = receipts.claim(&request, released, Duration::from_secs(60)).await?;
    receipts.confirm_absent(&request, claimed).await?;
    let done = receipts.load(&request).await?.ok_or("receipt")?;
    assert!(done.absent());
    assert!(matches!(
        receipts.claim(&request, done, Duration::from_secs(60)).await,
        Err(LeaseCleanupError::InProgress)
    ));
    Ok(())
}

fn key_for(request: &CancelOwnedLeaseRequest) -> Result<String, Box<dyn std::error::Error>> {
    Ok(super::receipts::key(request)?)
}

#[tokio::test]
async fn a_receipt_for_another_owner_is_refused() -> TestResult {
    let (server, receipts) = configured().await?;
    let first = request("l1", "sbx1")?;
    let second = request("l2", "sbx2")?;
    receipts.begin(&first).await?;
    let kv = server.jetstream().await.get_key_value(BUCKET).await?;
    let entry = kv.get(key_for(&first)?).await?.ok_or("receipt")?;
    kv.put(key_for(&second)?, entry).await?;
    assert!(matches!(
        receipts.load(&second).await,
        Err(LeaseCleanupError::OwnerMismatch)
    ));
    Ok(())
}

#[tokio::test]
async fn unreadable_records_are_refused_or_ignored() -> TestResult {
    let (server, receipts) = configured().await?;
    let request = request("l1", "sbx1")?;
    assert!(receipts.load(&request).await?.is_none());
    let kv = server.jetstream().await.get_key_value(BUCKET).await?;
    let key = key_for(&request)?;
    kv.put(&key, "not json".into()).await?;
    assert!(matches!(
        receipts.load(&request).await,
        Err(LeaseCleanupError::Decode(_))
    ));
    kv.put(&key, vec![b' '; 5000].into()).await?;
    assert!(matches!(
        receipts.load(&request).await,
        Err(LeaseCleanupError::TooLarge)
    ));
    kv.delete(&key).await?;
    assert!(receipts.load(&request).await?.is_none());
    Ok(())
}

#[tokio::test]
async fn an_absent_receipt_with_a_claim_is_refused() -> TestResult {
    let (server, receipts) = configured().await?;
    let request = request("l1", "sbx1")?;
    assert!(receipts.load(&request).await?.is_none());
    let kv = server.jetstream().await.get_key_value(BUCKET).await?;
    let body = serde_json::json!({
        "version": 1,
        "tenant": "t",
        "lease_id": "l1",
        "sandbox_id": "sbx1",
        "phase": "absent",
        "claim_until_unix_ms": 5,
    });
    kv.put(key_for(&request)?, serde_json::to_vec(&body)?.into()).await?;
    assert!(matches!(
        receipts.load(&request).await,
        Err(LeaseCleanupError::InvalidClaim)
    ));
    let body = serde_json::json!({
        "version": 2,
        "tenant": "t",
        "lease_id": "l1",
        "sandbox_id": "sbx1",
        "phase": "cleanup",
        "claim_until_unix_ms": 0,
    });
    kv.put(key_for(&request)?, serde_json::to_vec(&body)?.into()).await?;
    assert!(matches!(
        receipts.load(&request).await,
        Err(LeaseCleanupError::OwnerMismatch)
    ));
    Ok(())
}

#[tokio::test]
async fn a_bucket_made_unsafe_after_it_was_opened_is_refused() -> TestResult {
    let (server, receipts) = configured().await?;
    let request = request("l1", "sbx1")?;
    assert!(receipts.load(&request).await?.is_none());
    let context = server.jetstream().await;
    let mut config = context
        .get_stream(format!("KV_{BUCKET}"))
        .await?
        .cached_info()
        .config
        .clone();
    config.max_age = Duration::from_secs(600);
    context.update_stream(config).await?;
    assert!(matches!(
        receipts.load(&request).await,
        Err(LeaseCleanupError::Bucket(crate::CustodyBucketError::NotDurable))
    ));
    Ok(())
}

#[tokio::test]
async fn a_bucket_that_already_expires_records_is_refused_on_open() -> TestResult {
    let server = JetStreamTestServer::start().await;
    let context = server.jetstream().await;
    context
        .create_key_value(async_nats::jetstream::kv::Config {
            bucket: BUCKET.into(),
            history: 1,
            max_age: Duration::from_secs(600),
            ..Default::default()
        })
        .await?;
    let receipts = LeaseCleanupReceipts::default();
    receipts.configure(context)?;
    assert!(matches!(
        receipts.load(&request("l1", "sbx1")?).await,
        Err(LeaseCleanupError::Bucket(crate::CustodyBucketError::NotDurable))
    ));
    Ok(())
}

#[test]
fn a_receipt_record_may_not_outgrow_its_limit() {
    assert!(within_limit(MAX_RECORD_BYTES).is_ok());
    assert!(matches!(
        within_limit(MAX_RECORD_BYTES + 1),
        Err(LeaseCleanupError::TooLarge)
    ));
}

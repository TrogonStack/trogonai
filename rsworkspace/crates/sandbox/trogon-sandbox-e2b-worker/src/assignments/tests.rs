use base64::Engine;
use trogon_nats::test_support::JetStreamTestServer;
use trogon_sandbox_contract::{
    DispatchAttemptId, DispatchTarget, LeaseAttemptIdentity, LeaseEpoch, LeaseId, Port, ProviderKind, TenantId,
    WorkerId,
};

use super::*;
use crate::assignments::constants::MAX_RECORD_BYTES;

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn identity(lease: &str, epoch: Option<LeaseEpoch>) -> Result<LeaseAttemptIdentity, Box<dyn std::error::Error>> {
    Ok(LeaseAttemptIdentity {
        lease_id: LeaseId::new(lease),
        provider: ProviderKind::E2b,
        target: DispatchTarget {
            worker_id: WorkerId::new("w1")?,
            attempt_id: DispatchAttemptId::new("a1")?,
            epoch,
        },
    })
}

fn exact(lease: &str) -> Result<LeaseAttemptIdentity, Box<dyn std::error::Error>> {
    identity(lease, Some(LeaseEpoch::FIRST))
}

fn recovery(ttl_seconds: u32, region: Option<&str>) -> Result<Recovery, Box<dyn std::error::Error>> {
    Ok(Recovery {
        exposed_ports: vec![Port::new(3000)?],
        ttl_seconds,
        region: region.map(str::to_owned),
    })
}

async fn configured() -> Result<(JetStreamTestServer, Assignments), Box<dyn std::error::Error>> {
    let server = JetStreamTestServer::start().await;
    let store = Assignments::default();
    store.configure(server.jetstream().await).await?;
    Ok((server, store))
}

#[tokio::test]
async fn an_unconfigured_store_refuses_work() -> TestResult {
    let store = Assignments::default();
    assert!(matches!(
        store.load(&exact("l1")?).await,
        Err(AssignmentsError::NotConfigured)
    ));
    Ok(())
}

#[tokio::test]
async fn a_store_configures_once() -> TestResult {
    let (server, store) = configured().await?;
    assert!(matches!(
        store.configure(server.jetstream().await).await,
        Err(AssignmentsError::AlreadyConfigured)
    ));
    Ok(())
}

#[tokio::test]
async fn a_reservation_is_recorded_and_returned_again() -> TestResult {
    let (_server, store) = configured().await?;
    let id = exact("l1")?;
    assert!(store.load(&id).await?.is_none());
    let tenant = TenantId::new("t")?;
    let first = store.reserve(&tenant, &id, None).await?;
    assert_eq!(first.tenant, tenant);
    assert!(first.sandbox_id.is_none());
    let again = store.reserve(&tenant, &id, None).await?;
    assert_eq!(again.identity, id);
    assert!(store.load(&id).await?.is_some());
    Ok(())
}

#[tokio::test]
async fn another_tenant_cannot_take_a_reservation() -> TestResult {
    let (_server, store) = configured().await?;
    let id = exact("l1")?;
    store.reserve(&TenantId::new("t")?, &id, None).await?;
    assert!(matches!(
        store.reserve(&TenantId::new("other")?, &id, None).await,
        Err(AssignmentsError::TenantChanged)
    ));
    Ok(())
}

#[tokio::test]
async fn recovery_is_added_once_and_may_not_change() -> TestResult {
    let (_server, store) = configured().await?;
    let id = exact("l1")?;
    let tenant = TenantId::new("t")?;
    store.reserve(&tenant, &id, None).await?;
    let added = store.reserve(&tenant, &id, Some(recovery(600, Some("us"))?)).await?;
    assert!(added.recovery.is_some());
    let same = store.reserve(&tenant, &id, Some(recovery(600, Some("us"))?)).await?;
    assert!(same.recovery.is_some());
    assert!(matches!(
        store.reserve(&tenant, &id, Some(recovery(700, None)?)).await,
        Err(AssignmentsError::RecoveryChanged)
    ));
    Ok(())
}

#[tokio::test]
async fn a_native_sandbox_binds_once() -> TestResult {
    let (_server, store) = configured().await?;
    let id = exact("l1")?;
    let tenant = TenantId::new("t")?;
    assert!(matches!(
        store.bind_native(&id, "sbx1").await,
        Err(AssignmentsError::LacksOwnership)
    ));
    store.reserve(&tenant, &id, Some(recovery(600, None)?)).await?;
    store.bind_native(&id, "sbx1").await?;
    store.bind_native(&id, "sbx1").await?;
    assert!(matches!(
        store.bind_native(&id, "sbx2").await,
        Err(AssignmentsError::NativeIdentityChanged)
    ));
    assert!(matches!(
        store.bind_native(&id, "not/canonical").await,
        Err(AssignmentsError::SandboxId(_))
    ));
    let loaded = store.load(&id).await?.ok_or("assignment")?;
    assert_eq!(loaded.sandbox_id.as_deref(), Some("sbx1"));
    Ok(())
}

#[tokio::test]
async fn an_inexact_identity_is_refused() -> TestResult {
    let (_server, store) = configured().await?;
    let id = identity("l1", None)?;
    assert!(matches!(
        store.reserve(&TenantId::new("t")?, &id, None).await,
        Err(AssignmentsError::NotExact)
    ));
    Ok(())
}

#[tokio::test]
async fn a_record_that_is_not_an_assignment_is_refused() -> TestResult {
    let (server, store) = configured().await?;
    let id = exact("l1")?;
    let kv = server
        .jetstream()
        .await
        .get_key_value("sandbox-e2b-assignments")
        .await?;
    let key = base64_key(&id)?;
    kv.put(&key, "not json".into()).await?;
    assert!(matches!(store.load(&id).await, Err(AssignmentsError::Codec(_))));
    kv.delete(&key).await?;
    assert!(matches!(store.load(&id).await, Err(AssignmentsError::UnreadableMarker)));
    Ok(())
}

#[tokio::test]
async fn a_record_stored_under_the_wrong_identity_is_refused() -> TestResult {
    let (server, store) = configured().await?;
    let id = exact("l1")?;
    let other = exact("l2")?;
    store.reserve(&TenantId::new("t")?, &other, None).await?;
    let kv = server
        .jetstream()
        .await
        .get_key_value("sandbox-e2b-assignments")
        .await?;
    let entry = kv.get(base64_key(&other)?).await?.ok_or("record")?;
    kv.put(base64_key(&id)?, entry).await?;
    assert!(matches!(store.load(&id).await, Err(AssignmentsError::IdentityChanged)));
    Ok(())
}

fn base64_key(id: &LeaseAttemptIdentity) -> Result<String, Box<dyn std::error::Error>> {
    Ok(base64::engine::general_purpose::URL_SAFE_NO_PAD
        .encode(serde_json::to_vec(&(&id.target.worker_id, &id.lease_id))?))
}

#[test]
fn recovery_bounds_are_enforced() -> TestResult {
    assert!(recovery(600, None)?.validate().is_ok());
    assert!(matches!(
        recovery(0, None)?.validate(),
        Err(AssignmentsError::InvalidLifetime)
    ));
    assert!(matches!(
        recovery(u32::MAX, None)?.validate(),
        Err(AssignmentsError::InvalidLifetime)
    ));
    assert!(matches!(
        recovery(600, Some(""))?.validate(),
        Err(AssignmentsError::InvalidRegion)
    ));
    assert!(matches!(
        recovery(600, Some("a\nb"))?.validate(),
        Err(AssignmentsError::InvalidRegion)
    ));
    Ok(())
}

#[test]
fn a_record_that_outgrows_its_limit_is_refused() -> TestResult {
    let tenant = TenantId::new("t1")?;
    let assignment = Assignment::reserved(tenant, exact(&"l".repeat(MAX_RECORD_BYTES))?, None);
    assert!(matches!(assignment.bytes(), Err(AssignmentsError::TooLarge)));
    Ok(())
}

#[test]
fn a_sandbox_identity_must_be_canonical() {
    assert!(validate_sandbox_id("sbx-1").is_ok());
    assert!(validate_sandbox_id("").is_err());
}

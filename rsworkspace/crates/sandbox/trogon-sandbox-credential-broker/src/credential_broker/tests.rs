use std::error::Error;
use std::sync::Arc;

use trogon_sandbox_contract::{CredentialId, CredentialProvider, TenantId};

use super::*;
use crate::legacy_seed::seed_legacy;
use crate::{InMemoryAuditLogger, InMemoryCredentialStore, InMemoryTokenStore, InProcessKey};

const REGISTRY: CredentialProvider = CredentialProvider::Registry;

fn legacy_broker() -> (CredentialBroker, Arc<InMemoryCredentialStore>) {
    let store = Arc::new(InMemoryCredentialStore::new(Arc::new(InProcessKey::from_bytes(
        [1u8; 32],
    ))));
    let broker = CredentialBroker::new(
        store.clone(),
        Arc::new(InMemoryTokenStore::new()),
        Arc::new(InMemoryAuditLogger::new()),
    );
    (broker, store)
}

fn material(password: &str) -> CredentialMaterial {
    CredentialMaterial::RegistryBasic {
        username: "u".to_string(),
        password: password.to_string(),
    }
}

fn id(value: &str) -> CredentialId {
    CredentialId::new(value)
}

#[tokio::test]
async fn a_dry_run_writes_nothing() -> Result<(), Box<dyn Error>> {
    let (broker, store) = legacy_broker();
    seed_legacy(&store, REGISTRY, "c1", &material("a")).await?;
    seed_legacy(&store, REGISTRY, "c2", &material("b")).await?;
    let report = broker.migrate_legacy(REGISTRY, &TenantId::new("t")?, &[], true).await?;
    assert_eq!(report.would_migrate.len(), 2);
    assert!(report.migrated.is_empty());
    assert_eq!(report.remaining, 2);
    Ok(())
}

#[tokio::test]
async fn migration_rewrites_every_record_and_is_idempotent() -> Result<(), Box<dyn Error>> {
    let (broker, store) = legacy_broker();
    seed_legacy(&store, REGISTRY, "c1", &material("a")).await?;
    seed_legacy(&store, REGISTRY, "c2", &material("b")).await?;
    let tenant = TenantId::new("t")?;
    let report = broker.migrate_legacy(REGISTRY, &tenant, &[], false).await?;
    assert_eq!(report.migrated.len(), 2);
    assert!(report.failed.is_empty());
    assert_eq!(report.remaining, 0);
    let again = broker.migrate_legacy(REGISTRY, &tenant, &[], false).await?;
    assert!(again.migrated.is_empty() && again.remaining == 0);
    Ok(())
}

#[tokio::test]
async fn migration_resumes_after_a_partial_run() -> Result<(), Box<dyn Error>> {
    let (broker, store) = legacy_broker();
    seed_legacy(&store, REGISTRY, "c1", &material("a")).await?;
    seed_legacy(&store, REGISTRY, "c2", &material("b")).await?;
    let tenant = TenantId::new("t")?;
    store.reencrypt_credential(REGISTRY, Some(&tenant), &id("c1")).await?;
    let report = broker.migrate_legacy(REGISTRY, &tenant, &[], false).await?;
    assert_eq!(report.migrated, vec![id("c2")]);
    assert_eq!(report.remaining, 0);
    Ok(())
}

#[tokio::test]
async fn migration_takes_only_the_named_ids_for_each_tenant() -> Result<(), Box<dyn Error>> {
    let (broker, store) = legacy_broker();
    seed_legacy(&store, REGISTRY, "c1", &material("a")).await?;
    seed_legacy(&store, REGISTRY, "c2", &material("b")).await?;
    let (t1, t2) = (TenantId::new("t1")?, TenantId::new("t2")?);
    let report = broker.migrate_legacy(REGISTRY, &t1, &[id("c1")], false).await?;
    assert_eq!(report.migrated, vec![id("c1")]);
    assert_eq!(report.remaining, 1);
    let report = broker.migrate_legacy(REGISTRY, &t2, &[id("c2")], false).await?;
    assert_eq!(report.migrated, vec![id("c2")]);
    assert_eq!(report.remaining, 0);
    assert!(store.get_material(REGISTRY, &t1, &id("c1")).await.is_ok());
    assert!(store.get_material(REGISTRY, &t2, &id("c2")).await.is_ok());
    assert!(store.get_material(REGISTRY, &t1, &id("c2")).await.is_err());
    Ok(())
}

#[tokio::test]
async fn a_record_that_cannot_be_rewritten_is_reported_not_fatal() -> Result<(), Box<dyn Error>> {
    let store =
        Arc::new(InMemoryCredentialStore::new(Arc::new(InProcessKey::from_bytes([1u8; 32]))).with_legacy_reads(false));
    let broker = CredentialBroker::new(
        store.clone(),
        Arc::new(InMemoryTokenStore::new()),
        Arc::new(InMemoryAuditLogger::new()),
    );
    seed_legacy(&store, REGISTRY, "c1", &material("a")).await?;
    let report = broker
        .migrate_legacy(REGISTRY, &TenantId::new("t")?, &[], false)
        .await?;
    assert!(report.migrated.is_empty());
    assert_eq!(report.failed.len(), 1);
    assert_eq!(report.failed[0].0, id("c1"));
    assert_eq!(report.remaining, 1);
    Ok(())
}

use std::error::Error;
use std::sync::Arc;

use trogon_sandbox_contract::{CredentialId, CredentialProvider, PullSecretId, TenantId};

use super::*;
use crate::InProcessKey;
use crate::legacy_seed::seed_legacy;

const REGISTRY: CredentialProvider = CredentialProvider::Registry;

fn store() -> InMemoryCredentialStore {
    InMemoryCredentialStore::new(Arc::new(InProcessKey::from_bytes([3u8; 32])))
}

fn material(password: &str) -> CredentialMaterial {
    CredentialMaterial::RegistryBasic {
        username: "u".to_string(),
        password: password.to_string(),
    }
}

fn tenant(value: &str) -> Result<TenantId, Box<dyn Error>> {
    Ok(TenantId::new(value)?)
}

fn id(value: &str) -> CredentialId {
    CredentialId::new(value)
}

fn key(value: &str) -> (CredentialProvider, CredentialId) {
    (REGISTRY, id(value))
}

#[tokio::test]
async fn a_record_opens_for_its_owning_tenant() -> Result<(), Box<dyn Error>> {
    let s = store();
    assert_eq!(
        s.put_credential(REGISTRY, &tenant("a")?, &id("c1"), &material("pw"))
            .await?,
        1
    );
    assert_eq!(
        s.get_material(REGISTRY, &tenant("a")?, &id("c1")).await?,
        material("pw")
    );
    Ok(())
}

#[tokio::test]
async fn a_missing_record_is_not_found() -> Result<(), Box<dyn Error>> {
    let s = store();
    let result = s.get_material(REGISTRY, &tenant("a")?, &id("c1")).await;
    assert!(matches!(result, Err(CredentialStoreError::NotFound)));
    let result = s.reencrypt_credential(REGISTRY, None, &id("c1")).await;
    assert!(matches!(result, Err(CredentialStoreError::NotFound)));
    Ok(())
}

#[tokio::test]
async fn another_tenant_cannot_open_the_record() -> Result<(), Box<dyn Error>> {
    let s = store();
    s.put_credential(REGISTRY, &tenant("a")?, &id("c1"), &material("pw"))
        .await?;
    assert!(s.get_material(REGISTRY, &tenant("b")?, &id("c1")).await.is_err());
    Ok(())
}

#[tokio::test]
async fn a_record_copied_to_another_tenants_entry_does_not_open() -> Result<(), Box<dyn Error>> {
    let s = store();
    s.put_credential(REGISTRY, &tenant("a")?, &id("c1"), &material("a-pw"))
        .await?;
    s.put_credential(REGISTRY, &tenant("b")?, &id("c2"), &material("b-pw"))
        .await?;
    {
        let mut guard = s.credentials.lock().await;
        let copied = guard.get(&key("c1")).cloned().ok_or("missing")?;
        guard.insert(key("c2"), copied);
    }
    assert!(s.get_material(REGISTRY, &tenant("b")?, &id("c2")).await.is_err());
    {
        let mut guard = s.credentials.lock().await;
        let record = guard.get_mut(&key("c2")).ok_or("missing")?;
        record.envelope.as_mut().ok_or("no envelope")?.tenant = tenant("b")?;
    }
    assert!(s.get_material(REGISTRY, &tenant("b")?, &id("c2")).await.is_err());
    Ok(())
}

#[tokio::test]
async fn a_record_copied_under_another_credential_id_does_not_open() -> Result<(), Box<dyn Error>> {
    let s = store();
    s.put_credential(REGISTRY, &tenant("a")?, &id("c1"), &material("pw"))
        .await?;
    {
        let mut guard = s.credentials.lock().await;
        let copied = guard.get(&key("c1")).cloned().ok_or("missing")?;
        guard.insert(key("other"), copied);
    }
    assert!(s.get_material(REGISTRY, &tenant("a")?, &id("other")).await.is_err());
    Ok(())
}

#[tokio::test]
async fn a_record_read_as_another_provider_does_not_open() -> Result<(), Box<dyn Error>> {
    let s = store();
    s.put_credential(REGISTRY, &tenant("a")?, &id("c1"), &material("pw"))
        .await?;
    {
        let mut guard = s.credentials.lock().await;
        let copied = guard.get(&key("c1")).cloned().ok_or("missing")?;
        guard.insert((CredentialProvider::Daytona, id("c1")), copied);
    }
    let result = s
        .get_material(CredentialProvider::Daytona, &tenant("a")?, &id("c1"))
        .await;
    assert!(result.is_err());
    Ok(())
}

#[tokio::test]
async fn a_tampered_version_does_not_open() -> Result<(), Box<dyn Error>> {
    let s = store();
    s.put_credential(REGISTRY, &tenant("a")?, &id("c1"), &material("pw"))
        .await?;
    s.credentials.lock().await.get_mut(&key("c1")).ok_or("missing")?.version = 9;
    assert!(s.get_material(REGISTRY, &tenant("a")?, &id("c1")).await.is_err());
    Ok(())
}

#[tokio::test]
async fn a_record_without_its_envelope_is_refused() -> Result<(), Box<dyn Error>> {
    let s = store();
    s.put_credential(REGISTRY, &tenant("a")?, &id("c1"), &material("pw"))
        .await?;
    s.credentials
        .lock()
        .await
        .get_mut(&key("c1"))
        .ok_or("missing")?
        .envelope = None;
    let result = s.get_material(REGISTRY, &tenant("a")?, &id("c1")).await;
    assert!(matches!(result, Err(CredentialStoreError::MissingEnvelope)));
    Ok(())
}

#[tokio::test]
async fn a_legacy_record_reads_while_legacy_reads_are_on() -> Result<(), Box<dyn Error>> {
    let s = store();
    seed_legacy(&s, REGISTRY, "c1", &material("old")).await?;
    assert_eq!(
        s.get_material(REGISTRY, &tenant("a")?, &id("c1")).await?,
        material("old")
    );
    assert_eq!(s.list_legacy(REGISTRY).await?, vec![id("c1")]);
    Ok(())
}

#[tokio::test]
async fn a_legacy_record_is_refused_once_legacy_reads_are_off() -> Result<(), Box<dyn Error>> {
    let s = store().with_legacy_reads(false);
    seed_legacy(&s, REGISTRY, "c1", &material("old")).await?;
    let result = s.get_material(REGISTRY, &tenant("a")?, &id("c1")).await;
    assert!(matches!(result, Err(CredentialStoreError::LegacyDisabled)));
    Ok(())
}

#[tokio::test]
async fn reencrypting_a_legacy_record_needs_a_tenant() -> Result<(), Box<dyn Error>> {
    let s = store();
    seed_legacy(&s, REGISTRY, "c1", &material("old")).await?;
    let result = s.reencrypt_credential(REGISTRY, None, &id("c1")).await;
    assert!(matches!(result, Err(CredentialStoreError::LegacyTenantRequired)));
    assert_eq!(
        s.reencrypt_credential(REGISTRY, Some(&tenant("a")?), &id("c1")).await?,
        2
    );
    assert!(s.list_legacy(REGISTRY).await?.is_empty());
    assert_eq!(
        s.get_material(REGISTRY, &tenant("a")?, &id("c1")).await?,
        material("old")
    );
    Ok(())
}

#[tokio::test]
async fn reencrypting_an_envelope_record_uses_its_own_tenant() -> Result<(), Box<dyn Error>> {
    let s = store();
    s.put_credential(REGISTRY, &tenant("a")?, &id("c1"), &material("pw"))
        .await?;
    let result = s.reencrypt_credential(REGISTRY, Some(&tenant("b")?), &id("c1")).await;
    assert!(matches!(result, Err(CredentialStoreError::TenantMismatch)));
    assert_eq!(s.reencrypt_credential(REGISTRY, None, &id("c1")).await?, 2);
    assert_eq!(
        s.get_material(REGISTRY, &tenant("a")?, &id("c1")).await?,
        material("pw")
    );
    Ok(())
}

#[tokio::test]
async fn rotating_bumps_the_version_and_replaces_the_material() -> Result<(), Box<dyn Error>> {
    let s = store();
    assert_eq!(
        s.rotate_credential(REGISTRY, &tenant("a")?, &id("c1"), &material("v1"))
            .await?,
        1
    );
    assert_eq!(
        s.rotate_credential(REGISTRY, &tenant("a")?, &id("c1"), &material("v2"))
            .await?,
        2
    );
    assert_eq!(
        s.get_material(REGISTRY, &tenant("a")?, &id("c1")).await?,
        material("v2")
    );
    Ok(())
}

#[tokio::test]
async fn a_pull_secret_resolves_to_its_mapping_or_to_the_same_id() -> Result<(), Box<dyn Error>> {
    let s = store();
    let pull = PullSecretId::new("ps");
    assert_eq!(s.resolve_pull_secret(REGISTRY, &pull).await?, id("ps"));
    s.map_pull_secret(REGISTRY, &pull, &id("c9")).await?;
    assert_eq!(s.resolve_pull_secret(REGISTRY, &pull).await?, id("c9"));
    assert_eq!(s.resolve_pull_secret(CredentialProvider::S3, &pull).await?, id("ps"));
    Ok(())
}

#[tokio::test]
async fn listing_names_only_the_providers_credentials() -> Result<(), Box<dyn Error>> {
    let s = store();
    s.put_credential(REGISTRY, &tenant("a")?, &id("c1"), &material("pw"))
        .await?;
    s.put_credential(CredentialProvider::S3, &tenant("a")?, &id("c2"), &material("pw"))
        .await?;
    let listed = s.list_credentials(REGISTRY).await?;
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].credential_id, id("c1"));
    assert_eq!(listed[0].version, 1);
    Ok(())
}
